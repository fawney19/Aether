use std::future::Future;
use std::time::Duration;

pub async fn wait_until<F, Fut>(
    timeout: Duration,
    poll_interval: Duration,
    mut predicate: F,
) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::wait_until;

    #[tokio::test]
    async fn returns_true_when_predicate_eventually_passes() {
        let flag = Arc::new(AtomicBool::new(false));
        let background = flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            background.store(true, Ordering::Release);
        });

        let ready = wait_until(Duration::from_millis(100), Duration::from_millis(5), || {
            let flag = flag.clone();
            async move { flag.load(Ordering::Acquire) }
        })
        .await;

        assert!(ready);
    }
}
