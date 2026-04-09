use super::{AdminAppState, AdminCancelVideoTaskError};
use crate::GatewayError;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn list_gemini_file_mappings(
        &self,
        query: &aether_data::repository::gemini_file_mappings::GeminiFileMappingListQuery,
    ) -> Result<
        aether_data::repository::gemini_file_mappings::StoredGeminiFileMappingListPage,
        GatewayError,
    > {
        self.app.list_gemini_file_mappings(query).await
    }

    pub(crate) async fn summarize_gemini_file_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::gemini_file_mappings::GeminiFileMappingStats, GatewayError>
    {
        self.app.summarize_gemini_file_mappings(now_unix_secs).await
    }

    pub(crate) async fn delete_gemini_file_mapping_by_id(
        &self,
        mapping_id: &str,
    ) -> Result<
        Option<aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
        GatewayError,
    > {
        self.app.delete_gemini_file_mapping_by_id(mapping_id).await
    }

    pub(crate) async fn delete_expired_gemini_file_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<usize, GatewayError> {
        self.app
            .delete_expired_gemini_file_mappings(now_unix_secs)
            .await
    }

    pub(crate) async fn count_distinct_video_task_users(
        &self,
        filter: &aether_data_contracts::repository::video_tasks::VideoTaskQueryFilter,
    ) -> Result<u64, GatewayError> {
        self.app.count_distinct_video_task_users(filter).await
    }

    pub(crate) async fn read_video_task_page(
        &self,
        filter: &aether_data_contracts::repository::video_tasks::VideoTaskQueryFilter,
        page: usize,
        page_size: usize,
    ) -> Result<crate::async_task::VideoTaskPageResponse, GatewayError> {
        crate::async_task::read_video_task_page(self.app, filter, page, page_size).await
    }

    pub(crate) async fn read_video_task_stats(
        &self,
        filter: &aether_data_contracts::repository::video_tasks::VideoTaskQueryFilter,
        now_unix_secs: u64,
    ) -> Result<crate::async_task::VideoTaskStatsResponse, GatewayError> {
        crate::async_task::read_video_task_stats(self.app, filter, now_unix_secs).await
    }

    pub(crate) async fn cancel_video_task_record(
        &self,
        task_id: &str,
    ) -> Result<
        aether_data_contracts::repository::video_tasks::StoredVideoTask,
        AdminCancelVideoTaskError,
    > {
        crate::async_task::cancel_video_task_record(self.app, task_id)
            .await
            .map_err(|err| match err {
                crate::async_task::CancelVideoTaskError::NotFound => {
                    AdminCancelVideoTaskError::NotFound
                }
                crate::async_task::CancelVideoTaskError::InvalidStatus(status) => {
                    AdminCancelVideoTaskError::InvalidStatus(status)
                }
                crate::async_task::CancelVideoTaskError::Response(response) => {
                    AdminCancelVideoTaskError::Response(response)
                }
                crate::async_task::CancelVideoTaskError::Gateway(err) => {
                    AdminCancelVideoTaskError::Gateway(err)
                }
            })
    }

    pub(crate) async fn read_video_task_detail(
        &self,
        task_id: &str,
    ) -> Result<Option<aether_data_contracts::repository::video_tasks::StoredVideoTask>, GatewayError>
    {
        crate::async_task::read_video_task_detail(self.app, task_id).await
    }

    pub(crate) async fn read_video_task_video_source(
        &self,
        task_id: &str,
    ) -> Result<Option<crate::async_task::VideoTaskVideoSource>, GatewayError> {
        crate::async_task::read_video_task_video_source(self.app, task_id).await
    }

    pub(crate) async fn build_video_task_video_response(
        &self,
        task_id: &str,
        source: crate::async_task::VideoTaskVideoSource,
    ) -> Result<axum::response::Response, GatewayError> {
        crate::async_task::build_video_task_video_response(self.app, task_id, source).await
    }

    pub(crate) async fn store_local_gemini_file_mapping(
        &self,
        file_name: &str,
        key_id: &str,
        user_id: Option<&str>,
        display_name: Option<&str>,
        mime_type: Option<&str>,
    ) -> Result<(), GatewayError> {
        crate::usage::reporting::store_local_gemini_file_mapping(
            self.app,
            file_name,
            key_id,
            user_id,
            display_name,
            mime_type,
        )
        .await
    }
}
