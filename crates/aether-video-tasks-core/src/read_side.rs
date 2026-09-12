use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskLookupKey};
use aether_data_contracts::DataLayerError;
use async_trait::async_trait;

use crate::{
    map_gemini_stored_task_to_read_response, map_openai_stored_task_to_read_response,
    resolve_video_task_read_lookup_key, LocalVideoTaskReadResponse,
};

#[async_trait]
pub trait StoredVideoTaskReadSide: Send + Sync {
    async fn find_stored_video_task(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError>;

    async fn find_stored_video_task_for_user(
        &self,
        key: VideoTaskLookupKey<'_>,
        user_id: &str,
    ) -> Result<Option<StoredVideoTask>, DataLayerError>;
}

pub async fn read_data_backed_video_task_response(
    state: &impl StoredVideoTaskReadSide,
    route_family: Option<&str>,
    request_path: &str,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    read_data_backed_video_task_response_inner(state, route_family, request_path, None).await
}

pub async fn read_data_backed_video_task_response_for_user(
    state: &impl StoredVideoTaskReadSide,
    route_family: Option<&str>,
    request_path: &str,
    user_id: &str,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    let user_id = user_id.trim();
    if user_id.is_empty() {
        return Ok(None);
    }
    read_data_backed_video_task_response_inner(state, route_family, request_path, Some(user_id))
        .await
}

async fn read_data_backed_video_task_response_inner(
    state: &impl StoredVideoTaskReadSide,
    route_family: Option<&str>,
    request_path: &str,
    user_id: Option<&str>,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    match route_family {
        Some("openai") => read_openai_video_task_response(state, request_path, user_id).await,
        Some("gemini") => read_gemini_video_task_response(state, request_path, user_id).await,
        _ => Ok(None),
    }
}

async fn read_openai_video_task_response(
    state: &impl StoredVideoTaskReadSide,
    request_path: &str,
    user_id: Option<&str>,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    let Some(lookup) = resolve_video_task_read_lookup_key(Some("openai"), request_path) else {
        return Ok(None);
    };

    let task = match user_id {
        Some(user_id) => {
            state
                .find_stored_video_task_for_user(lookup, user_id)
                .await?
        }
        None => state.find_stored_video_task(lookup).await?,
    };
    let Some(task) = task else {
        return Ok(None);
    };

    if !matches!(task.provider_api_format.as_deref(), Some("openai:video")) {
        return Ok(None);
    }

    Ok(Some(map_openai_stored_task_to_read_response(task)))
}

async fn read_gemini_video_task_response(
    state: &impl StoredVideoTaskReadSide,
    request_path: &str,
    user_id: Option<&str>,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    let Some(lookup) = resolve_video_task_read_lookup_key(Some("gemini"), request_path) else {
        return Ok(None);
    };

    let task = match user_id {
        Some(user_id) => {
            state
                .find_stored_video_task_for_user(lookup, user_id)
                .await?
        }
        None => state.find_stored_video_task(lookup).await?,
    };
    let Some(task) = task else {
        return Ok(None);
    };

    if !matches!(task.provider_api_format.as_deref(), Some("gemini:video")) {
        return Ok(None);
    }

    Ok(Some(map_gemini_stored_task_to_read_response(task)))
}
