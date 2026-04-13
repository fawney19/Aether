use super::{AppState, GatewayError};

use crate::{async_task, video_tasks};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskStatusCount,
};

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

    pub(crate) async fn upsert_video_task_snapshot(
        &self,
        snapshot: &video_tasks::LocalVideoTaskSnapshot,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .upsert_video_task(snapshot.to_upsert_record())
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
