use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskLookupKey};
use aether_data_contracts::DataLayerError;
use async_trait::async_trait;

use crate::{
    map_doubao_stored_task_to_read_response, map_gemini_stored_task_to_read_response,
    map_openai_stored_task_to_read_response, resolve_video_task_read_lookup_key,
    LocalVideoTaskReadResponse,
};

#[async_trait]
pub trait StoredVideoTaskReadSide: Send + Sync {
    async fn find_stored_video_task(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError>;
}

pub async fn read_data_backed_video_task_response(
    state: &impl StoredVideoTaskReadSide,
    route_family: Option<&str>,
    request_path: &str,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    match route_family {
        Some("openai") => read_openai_video_task_response(state, request_path).await,
        Some("gemini") => read_gemini_video_task_response(state, request_path).await,
        Some("doubao") => read_doubao_video_task_response(state, request_path).await,
        _ => Ok(None),
    }
}

async fn read_openai_video_task_response(
    state: &impl StoredVideoTaskReadSide,
    request_path: &str,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    let Some(lookup) = resolve_video_task_read_lookup_key(Some("openai"), request_path) else {
        return Ok(None);
    };

    let Some(task) = state.find_stored_video_task(lookup).await? else {
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
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    let Some(lookup) = resolve_video_task_read_lookup_key(Some("gemini"), request_path) else {
        return Ok(None);
    };

    let Some(task) = state.find_stored_video_task(lookup).await? else {
        return Ok(None);
    };

    if !matches!(task.provider_api_format.as_deref(), Some("gemini:video")) {
        return Ok(None);
    }

    Ok(Some(map_gemini_stored_task_to_read_response(task)))
}

async fn read_doubao_video_task_response(
    state: &impl StoredVideoTaskReadSide,
    request_path: &str,
) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
    let Some(lookup) = resolve_video_task_read_lookup_key(Some("doubao"), request_path) else {
        return Ok(None);
    };

    let Some(task) = state.find_stored_video_task(lookup).await? else {
        return Ok(None);
    };

    if !matches!(task.provider_api_format.as_deref(), Some("doubao:video")) {
        return Ok(None);
    }

    Ok(Some(map_doubao_stored_task_to_read_response(task)))
}
