use std::future::Future;
use std::sync::OnceLock;

const USAGE_BACKGROUND_RUNTIME_THREADS: usize = 2;
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
            .worker_threads(USAGE_BACKGROUND_RUNTIME_THREADS)
            .thread_name(USAGE_BACKGROUND_RUNTIME_THREAD_NAME)
            .thread_stack_size(USAGE_BACKGROUND_RUNTIME_STACK_BYTES)
            .build()
            .expect("usage background runtime should build");
        Box::leak(Box::new(runtime))
    })
}

#[cfg(test)]
mod tests {
    use super::spawn_on_usage_background_runtime;

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
}
