use aether_runtime::{MetricKind, MetricSample};

pub(crate) fn gateway_tokio_runtime_metric_samples() -> Vec<MetricSample> {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return vec![
            availability_sample(0),
            gauge(
                "gateway_tokio_runtime_workers",
                TOKIO_RUNTIME_WORKERS_HELP,
                0,
            ),
            gauge(
                "gateway_tokio_runtime_alive_tasks",
                TOKIO_RUNTIME_ALIVE_TASKS_HELP,
                0,
            ),
            gauge(
                "gateway_tokio_runtime_global_queue_depth",
                TOKIO_RUNTIME_GLOBAL_QUEUE_DEPTH_HELP,
                0,
            ),
        ];
    };
    let metrics = handle.metrics();
    vec![
        availability_sample(1),
        gauge(
            "gateway_tokio_runtime_workers",
            TOKIO_RUNTIME_WORKERS_HELP,
            u64_from_usize(metrics.num_workers()),
        ),
        gauge(
            "gateway_tokio_runtime_alive_tasks",
            TOKIO_RUNTIME_ALIVE_TASKS_HELP,
            u64_from_usize(metrics.num_alive_tasks()),
        ),
        gauge(
            "gateway_tokio_runtime_global_queue_depth",
            TOKIO_RUNTIME_GLOBAL_QUEUE_DEPTH_HELP,
            u64_from_usize(metrics.global_queue_depth()),
        ),
    ]
}

const TOKIO_RUNTIME_WORKERS_HELP: &str =
    "Number of worker threads configured for the gateway Tokio runtime.";
const TOKIO_RUNTIME_ALIVE_TASKS_HELP: &str =
    "Current number of alive tasks tracked by the gateway Tokio runtime.";
const TOKIO_RUNTIME_GLOBAL_QUEUE_DEPTH_HELP: &str =
    "Current number of tasks waiting in the gateway Tokio runtime global queue.";

fn availability_sample(value: u64) -> MetricSample {
    gauge(
        "gateway_tokio_runtime_observability_available",
        "Whether gateway Tokio runtime metrics were available for this scrape.",
        value,
    )
}

fn gauge(name: &'static str, help: &'static str, value: u64) -> MetricSample {
    MetricSample::new(name, help, MetricKind::Gauge, value)
}

fn u64_from_usize(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::gateway_tokio_runtime_metric_samples;

    #[tokio::test]
    async fn renders_tokio_runtime_metrics_inside_runtime() {
        let samples = gateway_tokio_runtime_metric_samples();

        assert!(samples.iter().any(|sample| {
            sample.name == "gateway_tokio_runtime_observability_available" && sample.value == 1
        }));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_tokio_runtime_workers"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_tokio_runtime_alive_tasks"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_tokio_runtime_global_queue_depth"));
    }
}
