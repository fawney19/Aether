use super::{AppState, GatewayError};

use crate::{async_task, video_tasks};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskStatusCount,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoTaskRouteAccess {
    Allowed,
    NotFound,
    Denied,
}

impl AppState {
    pub(crate) async fn read_data_backed_video_task_response(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Result<Option<video_tasks::LocalVideoTaskReadResponse>, GatewayError> {
        self.data
            .read_video_task_response(route_family, request_path)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_data_backed_video_task_response_for_user(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        user_id: &str,
    ) -> Result<Option<video_tasks::LocalVideoTaskReadResponse>, GatewayError> {
        self.data
            .read_video_task_response_for_user(route_family, request_path, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_id(
        &self,
        task_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task(VideoTaskLookupKey::Id(task_id))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_short_id(
        &self,
        short_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task(VideoTaskLookupKey::ShortId(short_id))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_id_for_user(
        &self,
        task_id: &str,
        user_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task_for_user(VideoTaskLookupKey::Id(task_id), user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_short_id_for_user(
        &self,
        short_id: &str,
        user_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task_for_user(VideoTaskLookupKey::ShortId(short_id), user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn upsert_video_task_snapshot(
        &self,
        snapshot: &video_tasks::LocalVideoTaskSnapshot,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        let mut record = snapshot.to_upsert_record();
        // Reconstructed snapshots intentionally omit sensitive/request-only fields. Preserve the
        // persisted row's immutable identity and request-shape scalars before writing lifecycle
        // changes back, so the repository can continue enforcing immutable-field integrity.
        let existing_by_id = self
            .data
            .find_video_task(VideoTaskLookupKey::Id(record.id.as_str()))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let existing = if existing_by_id.is_some() {
            existing_by_id
        } else if let Some(short_id) = record.short_id.as_deref() {
            self.data
                .find_video_task(VideoTaskLookupKey::ShortId(short_id))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
        } else {
            None
        };
        if let Some(existing) = existing {
            record.id = existing.id;
            record.short_id = existing.short_id;
            record.request_id = existing.request_id;
            record.user_id = existing.user_id;
            record.api_key_id = existing.api_key_id;
            record.external_task_id = existing.external_task_id;
            record.provider_id = existing.provider_id;
            record.endpoint_id = existing.endpoint_id;
            record.key_id = existing.key_id;
            record.client_api_format = existing.client_api_format;
            record.provider_api_format = existing.provider_api_format;
            record.format_converted = existing.format_converted;
            record.model = existing.model;
            record.duration_seconds = existing.duration_seconds;
            record.resolution = existing.resolution;
            record.aspect_ratio = existing.aspect_ratio;
            record.size = existing.size;
        }
        self.data
            .upsert_video_task(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn hydrate_video_task_for_route(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Result<bool, GatewayError> {
        let lookup =
            video_tasks::resolve_video_task_hydration_lookup_key(route_family, request_path);
        let Some(lookup) = lookup else {
            return Ok(false);
        };
        let Some(task) = self
            .data
            .find_video_task(lookup)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        else {
            return Ok(false);
        };
        if self.video_tasks.hydrate_from_stored_task(&task) {
            return Ok(true);
        }

        let Some(snapshot) = self.reconstruct_video_task_snapshot(&task).await? else {
            return Ok(false);
        };
        self.video_tasks.record_snapshot(snapshot);
        Ok(true)
    }

    pub(crate) async fn hydrate_video_task_for_route_for_user(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        user_id: &str,
    ) -> Result<VideoTaskRouteAccess, GatewayError> {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Ok(VideoTaskRouteAccess::Denied);
        }
        let Some(lookup) =
            video_tasks::resolve_video_task_hydration_lookup_key(route_family, request_path)
        else {
            return Ok(VideoTaskRouteAccess::NotFound);
        };

        if let Some(task) = self
            .data
            .find_video_task_for_user(lookup, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        {
            if !self.video_tasks.hydrate_from_stored_task(&task) {
                if let Some(snapshot) = self.reconstruct_video_task_snapshot(&task).await? {
                    self.video_tasks.record_snapshot(snapshot);
                }
            }
            return Ok(VideoTaskRouteAccess::Allowed);
        }

        Ok(
            match self
                .video_tasks
                .snapshot_for_route(route_family, request_path)
            {
                Some(snapshot) if snapshot.belongs_to_user(user_id) => {
                    VideoTaskRouteAccess::Allowed
                }
                Some(_) => VideoTaskRouteAccess::Denied,
                None => VideoTaskRouteAccess::NotFound,
            },
        )
    }

    pub(crate) async fn reconstruct_video_task_snapshot(
        &self,
        task: &StoredVideoTask,
    ) -> Result<Option<video_tasks::LocalVideoTaskSnapshot>, GatewayError> {
        crate::provider_transport::reconstruct_local_video_task_snapshot(self, task)
            .await
            .map_err(GatewayError::Internal)
    }

    pub(crate) async fn claim_due_video_tasks(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, GatewayError> {
        self.data
            .claim_due_video_tasks(now_unix_secs, claim_until_unix_secs, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_active_video_task(
        &self,
        task: UpsertVideoTask,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .update_active_video_task(task)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_video_task_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, GatewayError> {
        self.data
            .list_video_task_page(filter, offset, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_video_task_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, GatewayError> {
        self.data
            .list_video_task_page_summary(filter, offset, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_video_tasks(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_video_tasks(filter)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_video_tasks_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, GatewayError> {
        self.data
            .count_video_tasks_by_status(filter)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_distinct_video_task_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_distinct_video_task_users(filter)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn top_video_task_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, GatewayError> {
        self.data
            .top_video_task_models(filter, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_video_tasks_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_video_tasks_created_since(filter, created_since_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn execute_video_task_refresh_plan(
        &self,
        refresh_plan: &video_tasks::LocalVideoTaskReadRefreshPlan,
    ) -> Result<bool, GatewayError> {
        async_task::execute_video_task_refresh_plan(self, refresh_plan).await
    }
}
