use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::task::{JoinError, JoinHandle};

pub(super) struct SessionTask<T>(JoinHandle<T>);

impl<T> SessionTask<T> {
    pub(super) fn new(handle: JoinHandle<T>) -> Self {
        Self(handle)
    }
    pub(super) fn abort(&self) {
        self.0.abort();
    }
    pub(super) fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl<T> Future for SessionTask<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(context)
    }
}

impl<T> Drop for SessionTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropping_a_session_task_aborts_its_child() {
        let child = tokio::spawn(std::future::pending::<()>());
        let abort = child.abort_handle();
        drop(SessionTask::new(child));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
