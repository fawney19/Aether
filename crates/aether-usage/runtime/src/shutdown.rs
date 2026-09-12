use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Debug, Default)]
pub(crate) struct UsageBackgroundTasks {
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl UsageBackgroundTasks {
    pub(crate) fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(crate::executor::spawn_on_usage_background_runtime(task));
    }

    pub(crate) async fn stop_idle(&self) {
        let handles = std::mem::take(
            &mut *self
                .handles
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for handle in &handles {
            handle.abort();
        }
        for handle in handles {
            let _ = handle.await;
        }
    }
}

impl Drop for UsageBackgroundTasks {
    fn drop(&mut self) {
        for handle in self.handles.get_mut().unwrap_or_else(|p| p.into_inner()) {
            handle.abort();
        }
    }
}

#[derive(Debug)]
pub(crate) struct UsageShutdownState {
    pub(crate) producers: Arc<AtomicUsize>,
    pub(crate) drain: watch::Sender<bool>,
    pub(crate) tasks: UsageBackgroundTasks,
    pub(crate) worker_control: crate::worker::UsageWorkerControl,
    pub(crate) supervisors: Arc<AtomicUsize>,
    pub(crate) lock: tokio::sync::Mutex<()>,
}

impl Default for UsageShutdownState {
    fn default() -> Self {
        Self {
            producers: Arc::new(AtomicUsize::new(0)),
            drain: watch::channel(false).0,
            tasks: UsageBackgroundTasks::default(),
            worker_control: crate::worker::UsageWorkerControl::default(),
            supervisors: Arc::new(AtomicUsize::new(0)),
            lock: tokio::sync::Mutex::new(()),
        }
    }
}

/// Retain across an owned request finalizer, including any detached handoff.
#[derive(Debug)]
pub struct UsageProducerGuard(pub(crate) Arc<AtomicUsize>);

impl Drop for UsageProducerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) async fn wait_for_drain(signal: &mut watch::Receiver<bool>) {
    loop {
        if *signal.borrow_and_update() {
            return;
        }
        if signal.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

pub(crate) async fn retry_delay(delay: Duration, signal: &mut watch::Receiver<bool>) {
    if *signal.borrow_and_update() {
        tokio::time::sleep(delay.min(Duration::from_millis(100))).await;
    } else {
        tokio::select! {
            _ = tokio::time::sleep(delay) => {},
            _ = wait_for_drain(signal) => {},
        }
    }
}
