use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    BackgroundTaskListQuery, BackgroundTaskReadRepository, BackgroundTaskStatus,
    BackgroundTaskSummary, BackgroundTaskWriteRepository, StoredBackgroundTaskEvent,
    StoredBackgroundTaskRun, StoredBackgroundTaskRunPage, UpsertBackgroundTaskEvent,
    UpsertBackgroundTaskRun,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
struct InMemoryBackgroundTaskIndex {
    runs: BTreeMap<String, StoredBackgroundTaskRun>,
    events_by_run: BTreeMap<String, Vec<StoredBackgroundTaskEvent>>,
}

#[derive(Debug, Default)]
pub struct InMemoryBackgroundTaskRepository {
    index: RwLock<InMemoryBackgroundTaskIndex>,
}

impl InMemoryBackgroundTaskRepository {
    fn matches_filter(run: &StoredBackgroundTaskRun, query: &BackgroundTaskListQuery) -> bool {
        if let Some(kind) = query.kind {
            if run.kind != kind {
                return false;
            }
        }
        if let Some(status) = query.status {
            if run.status != status {
                return false;
            }
        }
        if let Some(trigger) = query.trigger.as_deref() {
            if run.trigger != trigger {
                return false;
            }
        }
        if let Some(task_key_substring) = query.task_key_substring.as_deref() {
            let needle = task_key_substring.to_ascii_lowercase();
            if !run.task_key.to_ascii_lowercase().contains(&needle) {
                return false;
            }
        }
        true
    }

    pub fn seed_runs<I>(runs: I) -> Self
    where
        I: IntoIterator<Item = StoredBackgroundTaskRun>,
    {
        let mut index = InMemoryBackgroundTaskIndex::default();
        for run in runs {
            index.runs.insert(run.id.clone(), run);
        }
        Self {
            index: RwLock::new(index),
        }
    }
}

#[async_trait]
impl BackgroundTaskReadRepository for InMemoryBackgroundTaskRepository {
    async fn find_run(
        &self,
        run_id: &str,
    ) -> Result<Option<StoredBackgroundTaskRun>, DataLayerError> {
        Ok(self
            .index
            .read()
            .expect("background task repository lock")
            .runs
            .get(run_id)
            .cloned())
    }

    async fn list_runs(
        &self,
        query: &BackgroundTaskListQuery,
    ) -> Result<StoredBackgroundTaskRunPage, DataLayerError> {
        let mut items = self
            .index
            .read()
            .expect("background task repository lock")
            .runs
            .values()
            .filter(|run| Self::matches_filter(run, query))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .created_at_unix_secs
                .cmp(&left.created_at_unix_secs)
                .then_with(|| right.updated_at_unix_secs.cmp(&left.updated_at_unix_secs))
        });

        let total = items.len();
        let limit = query.limit.max(1);
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(limit)
            .collect::<Vec<_>>();
        Ok(StoredBackgroundTaskRunPage { items, total })
    }

    async fn list_events(
        &self,
        run_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredBackgroundTaskEvent>, DataLayerError> {
        let Some(events) = self
            .index
            .read()
            .expect("background task repository lock")
            .events_by_run
            .get(run_id)
            .cloned()
        else {
            return Ok(Vec::new());
        };
        let limit = limit.max(1);
        Ok(events.into_iter().skip(offset).take(limit).collect())
    }

    async fn summarize_runs(&self) -> Result<BackgroundTaskSummary, DataLayerError> {
        let runs = self
            .index
            .read()
            .expect("background task repository lock")
            .runs
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut by_status = BTreeMap::new();
        let mut by_kind = BTreeMap::new();
        let mut running_count = 0_u64;
        for run in runs {
            *by_status
                .entry(run.status.as_database().to_string())
                .or_insert(0) += 1;
            *by_kind
                .entry(run.kind.as_database().to_string())
                .or_insert(0) += 1;
            if run.status == BackgroundTaskStatus::Running {
                running_count += 1;
            }
        }
        let total = by_status.values().copied().sum();
        Ok(BackgroundTaskSummary {
            total,
            running_count,
            by_status,
            by_kind,
        })
    }
}

#[async_trait]
impl BackgroundTaskWriteRepository for InMemoryBackgroundTaskRepository {
    async fn upsert_run(
        &self,
        run: UpsertBackgroundTaskRun,
    ) -> Result<StoredBackgroundTaskRun, DataLayerError> {
        run.validate()?;
        let stored = run.into_stored();
        self.index
            .write()
            .expect("background task repository lock")
            .runs
            .insert(stored.id.clone(), stored.clone());
        Ok(stored)
    }

    async fn request_cancel(
        &self,
        run_id: &str,
        updated_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let mut guard = self.index.write().expect("background task repository lock");
        let Some(run) = guard.runs.get_mut(run_id) else {
            return Ok(false);
        };
        run.cancel_requested = true;
        run.updated_at_unix_secs = updated_at_unix_secs;
        Ok(true)
    }

    async fn upsert_event(
        &self,
        event: UpsertBackgroundTaskEvent,
    ) -> Result<StoredBackgroundTaskEvent, DataLayerError> {
        event.validate()?;
        let stored = event.into_stored();
        let mut guard = self.index.write().expect("background task repository lock");
        let entries = guard
            .events_by_run
            .entry(stored.run_id.clone())
            .or_default();
        if let Some(position) = entries.iter().position(|value| value.id == stored.id) {
            entries[position] = stored.clone();
        } else {
            entries.push(stored.clone());
        }
        let mut seen = BTreeSet::new();
        entries.retain(|entry| seen.insert(entry.id.clone()));
        entries.sort_by(|left, right| {
            left.created_at_unix_secs
                .cmp(&right.created_at_unix_secs)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(stored)
    }
}
