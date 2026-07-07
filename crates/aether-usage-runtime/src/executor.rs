use std::future::Future;
use std::sync::OnceLock;

const DEFAULT_USAGE_BACKGROUND_RUNTIME_THREADS: usize = 8;
const MAX_USAGE_BACKGROUND_RUNTIME_THREADS: usize = 64;
const GATEWAY_USAGE_BACKGROUND_RUNTIME_THREADS_ENV: &str = "AETHER_GATEWAY_USAGE_RUNTIME_THREADS";
const USAGE_BACKGROUND_RUNTIME_THREADS_ENV: &str = "AETHER_USAGE_RUNTIME_THREADS";
const USAGE_BACKGROUND_RUNTIME_STACK_BYTES: usize = 8 * 1024 * 1024;
const USAGE_BACKGROUND_RUNTIME_THREAD_NAME: &str = "aether-usage-runtime";

pub(crate) fn spawn_on_usage_background_runtime<F>(task: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    usage_background_runtime().handle().spawn(task)
}

fn usage_background_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<&'static tokio::runtime::Runtime> = OnceLock::new();

    RUNTIME.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(usage_background_runtime_threads())
            .thread_name(USAGE_BACKGROUND_RUNTIME_THREAD_NAME)
            .thread_stack_size(USAGE_BACKGROUND_RUNTIME_STACK_BYTES)
            .build()
            .expect("usage background runtime should build");
        Box::leak(Box::new(runtime))
    })
}

fn usage_background_runtime_threads() -> usize {
    parse_usage_background_runtime_threads(
        std::env::var(GATEWAY_USAGE_BACKGROUND_RUNTIME_THREADS_ENV)
            .ok()
            .or_else(|| std::env::var(USAGE_BACKGROUND_RUNTIME_THREADS_ENV).ok())
            .as_deref(),
    )
}

fn parse_usage_background_runtime_threads(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or(DEFAULT_USAGE_BACKGROUND_RUNTIME_THREADS)
        .clamp(1, MAX_USAGE_BACKGROUND_RUNTIME_THREADS)
}

#[cfg(test)]
mod tests {
    use super::{parse_usage_background_runtime_threads, spawn_on_usage_background_runtime};

    #[tokio::test]
    async fn usage_background_runtime_runs_on_dedicated_named_threads() {
        let thread_name = spawn_on_usage_background_runtime(async move {
            std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string()
        })
        .await
        .expect("background task should complete");

        assert_eq!(thread_name, "aether-usage-runtime");
    }

    #[test]
    fn usage_background_runtime_threads_are_configurable() {
        assert_eq!(parse_usage_background_runtime_threads(None), 8);
        assert_eq!(parse_usage_background_runtime_threads(Some("12")), 12);
        assert_eq!(parse_usage_background_runtime_threads(Some("0")), 8);
        assert_eq!(
            parse_usage_background_runtime_threads(Some("not-a-number")),
            8
        );
        assert_eq!(parse_usage_background_runtime_threads(Some("999")), 64);
    }
}
