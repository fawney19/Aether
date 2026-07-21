use std::future::Future;
use std::sync::OnceLock;

const DEFAULT_USAGE_BACKGROUND_RUNTIME_THREADS: usize = 8;
const MAX_USAGE_BACKGROUND_RUNTIME_THREADS: usize = 64;
const GATEWAY_USAGE_BACKGROUND_RUNTIME_THREADS_ENV: &str = "AETHER_GATEWAY_USAGE_RUNTIME_THREADS";
const USAGE_BACKGROUND_RUNTIME_THREADS_ENV: &str = "AETHER_USAGE_RUNTIME_THREADS";
const DEFAULT_USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS: usize = 16;
const MAX_USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS: usize = 128;
const GATEWAY_USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS_ENV: &str =
    "AETHER_GATEWAY_USAGE_RUNTIME_BLOCKING_THREADS";
const USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS_ENV: &str = "AETHER_USAGE_RUNTIME_BLOCKING_THREADS";
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
        let worker_threads = usage_background_runtime_threads();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(worker_threads)
            // Usage event builders use spawn_blocking; keep bursts from creating the Tokio
            // default pool of up to 512 threads and consuming an unbounded stack budget.
            .max_blocking_threads(usage_background_runtime_blocking_threads())
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

fn usage_background_runtime_blocking_threads() -> usize {
    parse_usage_background_runtime_blocking_threads(
        std::env::var(GATEWAY_USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS_ENV)
            .ok()
            .or_else(|| std::env::var(USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS_ENV).ok())
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

fn parse_usage_background_runtime_blocking_threads(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or(DEFAULT_USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS)
        .clamp(1, MAX_USAGE_BACKGROUND_RUNTIME_BLOCKING_THREADS)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        parse_usage_background_runtime_blocking_threads, parse_usage_background_runtime_threads,
        spawn_on_usage_background_runtime, usage_background_runtime_blocking_threads,
    };

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

    #[test]
    fn usage_background_runtime_blocking_threads_are_bounded() {
        assert_eq!(parse_usage_background_runtime_blocking_threads(None), 16);
        assert_eq!(
            parse_usage_background_runtime_blocking_threads(Some("0")),
            16
        );
        assert_eq!(
            parse_usage_background_runtime_blocking_threads(Some("32")),
            32
        );
        assert_eq!(
            parse_usage_background_runtime_blocking_threads(Some("999")),
            128
        );
        assert_eq!(
            parse_usage_background_runtime_blocking_threads(Some("not-a-number")),
            16
        );
    }

    #[tokio::test]
    async fn usage_background_runtime_does_not_exceed_blocking_thread_budget() {
        let configured_limit = usage_background_runtime_blocking_threads();
        let submissions = configured_limit.saturating_add(2).min(20);
        let expected_started = submissions.min(configured_limit);
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::with_capacity(submissions);

        for _ in 0..submissions {
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            handles.push(spawn_on_usage_background_runtime(async move {
                tokio::task::spawn_blocking(move || {
                    let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                    max_active.fetch_max(current, Ordering::AcqRel);
                    started.fetch_add(1, Ordering::AcqRel);
                    while !release.load(Ordering::Acquire) {
                        std::thread::park_timeout(Duration::from_millis(1));
                    }
                    active.fetch_sub(1, Ordering::AcqRel);
                })
                .await
                .expect("bounded blocking task should complete");
            }));
        }

        tokio::time::timeout(Duration::from_secs(5), async {
            while started.load(Ordering::Acquire) < expected_started {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("blocking pool should start tasks within the budget");

        assert!(
            max_active.load(Ordering::Acquire) <= configured_limit,
            "blocking pool exceeded configured limit: max={} limit={configured_limit}",
            max_active.load(Ordering::Acquire)
        );

        release.store(true, Ordering::Release);
        for handle in handles {
            tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("blocking task should stop after release")
                .expect("blocking task should not panic");
        }
    }
}
