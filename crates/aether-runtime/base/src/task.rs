use std::future::Future;

pub fn spawn_named<F>(task_name: &'static str, future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(async move {
        tracing::debug!(task = task_name, "spawned runtime task");
        future.await
    })
}
