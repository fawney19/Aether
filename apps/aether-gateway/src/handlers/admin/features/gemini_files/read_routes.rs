use super::{
    admin_gemini_files_error_response, admin_gemini_files_key_capable,
    admin_gemini_files_now_unix_secs, ADMIN_GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
    ADMIN_GEMINI_FILES_DEFAULT_PAGE, ADMIN_GEMINI_FILES_DEFAULT_PAGE_SIZE,
    ADMIN_GEMINI_FILES_MAX_PAGE_SIZE,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{
    admin_gemini_file_mapping_id_from_path, is_admin_gemini_files_capable_keys_root,
    is_admin_gemini_files_mappings_root, is_admin_gemini_files_stats_root,
    query_param_optional_bool, query_param_value, unix_secs_to_rfc3339,
};
use crate::GatewayError;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use axum::body::Body;
use axum::http::{self, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
struct AdminGeminiFilesPageQuery {
    page: usize,
    page_size: usize,
    include_expired: bool,
    search: Option<String>,
}

fn admin_gemini_files_page_query(
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<AdminGeminiFilesPageQuery>, GatewayError> {
    let query = request_context.query_string();
    let page = match query_param_value(query, "page") {
        Some(raw) => raw.parse::<usize>().ok().filter(|value| *value >= 1),
        None => Some(ADMIN_GEMINI_FILES_DEFAULT_PAGE),
    }
    .ok_or_else(|| {
        GatewayError::Internal("admin gemini files page query should validate".to_string())
    })?;
    let page_size = match query_param_value(query, "page_size") {
        Some(raw) => raw
            .parse::<usize>()
            .ok()
            .filter(|value| (1..=ADMIN_GEMINI_FILES_MAX_PAGE_SIZE).contains(value)),
        None => Some(ADMIN_GEMINI_FILES_DEFAULT_PAGE_SIZE),
    }
    .ok_or_else(|| {
        GatewayError::Internal("admin gemini files page_size query should validate".to_string())
    })?;
    let include_expired = query_param_optional_bool(query, "include_expired").unwrap_or(false);
    let search = query_param_value(query, "search").and_then(|value| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    });
    Ok(Some(AdminGeminiFilesPageQuery {
        page,
        page_size,
        include_expired,
        search,
    }))
}

async fn admin_gemini_files_key_name_map(
    state: &AdminAppState<'_>,
) -> Result<BTreeMap<String, String>, GatewayError> {
    let capable_keys = admin_gemini_files_all_keys(state).await?;
    Ok(capable_keys
        .into_iter()
        .map(|key| (key.id, key.name))
        .collect())
}

async fn admin_gemini_files_username_map<'a, I>(
    state: &AdminAppState<'_>,
    mappings: I,
) -> Result<BTreeMap<String, String>, GatewayError>
where
    I: Iterator<Item = &'a aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
{
    let user_ids = mappings
        .filter_map(|mapping| mapping.user_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let users = state.list_users_by_ids(&user_ids).await?;
    Ok(users
        .into_iter()
        .map(|user| (user.id, user.username))
        .collect())
}

async fn admin_gemini_files_capable_keys(
    state: &AdminAppState<'_>,
) -> Result<Vec<serde_json::Value>, GatewayError> {
    let providers = state.list_provider_catalog_providers(false).await?;
    let provider_name_by_id = providers
        .iter()
        .map(|provider| (provider.id.as_str(), provider.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let keys = admin_gemini_files_all_keys(state).await?;
    Ok(keys
        .into_iter()
        .filter(admin_gemini_files_key_capable)
        .map(|key| {
            json!({
                "id": key.id,
                "name": key.name,
                "provider_name": provider_name_by_id.get(key.provider_id.as_str()).copied(),
            })
        })
        .collect())
}

async fn admin_gemini_files_all_keys(
    state: &AdminAppState<'_>,
) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
    let providers = state.list_provider_catalog_providers(false).await?;
    let provider_ids = providers
        .into_iter()
        .map(|provider| provider.id)
        .collect::<Vec<_>>();
    state
        .list_provider_catalog_keys_by_provider_ids(&provider_ids)
        .await
}

fn build_admin_gemini_file_mapping_payload(
    mapping: &aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping,
    key_name: Option<&str>,
    username: Option<&str>,
    now_unix_secs: u64,
) -> serde_json::Value {
    json!({
        "id": mapping.id,
        "file_name": mapping.file_name,
        "key_id": mapping.key_id,
        "key_name": key_name,
        "user_id": mapping.user_id,
        "username": username,
        "display_name": mapping.display_name,
        "mime_type": mapping.mime_type,
        "created_at": unix_secs_to_rfc3339(mapping.created_at_unix_secs),
        "expires_at": unix_secs_to_rfc3339(mapping.expires_at_unix_secs),
        "is_expired": mapping.expires_at_unix_secs <= now_unix_secs,
    })
}

pub(super) async fn maybe_build_local_admin_gemini_files_read_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let now_unix_secs = admin_gemini_files_now_unix_secs();

    match request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
    {
        Some("list_mappings")
            if request_context.method() == http::Method::GET
                && is_admin_gemini_files_mappings_root(request_context.path()) =>
        {
            if !state.has_gemini_file_mapping_data_reader() {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ADMIN_GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
                )));
            }
            let page = match admin_gemini_files_page_query(request_context)? {
                Some(value) => value,
                None => return Ok(None),
            };
            let mappings = state
                .list_gemini_file_mappings(
                    &aether_data::repository::gemini_file_mappings::GeminiFileMappingListQuery {
                        include_expired: page.include_expired,
                        search: page.search.clone(),
                        offset: (page.page - 1).saturating_mul(page.page_size),
                        limit: page.page_size,
                        now_unix_secs,
                    },
                )
                .await?;
            let key_name_by_id = admin_gemini_files_key_name_map(state).await?;
            let username_by_id =
                admin_gemini_files_username_map(state, mappings.items.iter()).await?;
            let items = mappings
                .items
                .iter()
                .map(|mapping| {
                    build_admin_gemini_file_mapping_payload(
                        mapping,
                        key_name_by_id
                            .get(mapping.key_id.as_str())
                            .map(String::as_str),
                        username_by_id
                            .get(mapping.user_id.as_deref().unwrap_or(""))
                            .map(String::as_str),
                        now_unix_secs,
                    )
                })
                .collect::<Vec<_>>();
            Ok(Some(
                Json(json!({
                    "items": items,
                    "total": mappings.total,
                    "page": page.page,
                    "page_size": page.page_size,
                }))
                .into_response(),
            ))
        }
        Some("stats")
            if request_context.method() == http::Method::GET
                && is_admin_gemini_files_stats_root(request_context.path()) =>
        {
            if !state.has_gemini_file_mapping_data_reader() {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ADMIN_GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
                )));
            }
            let stats = state.summarize_gemini_file_mappings(now_unix_secs).await?;
            let capable_keys_count = admin_gemini_files_capable_keys(state).await?.len();
            let by_mime_type = stats
                .by_mime_type
                .into_iter()
                .map(|item| (item.mime_type, json!(item.count)))
                .collect::<serde_json::Map<_, _>>();
            Ok(Some(
                Json(json!({
                    "total_mappings": stats.total_mappings,
                    "active_mappings": stats.active_mappings,
                    "expired_mappings": stats.expired_mappings,
                    "by_mime_type": by_mime_type,
                    "capable_keys_count": capable_keys_count,
                }))
                .into_response(),
            ))
        }
        Some("delete_mapping")
            if request_context.method() == http::Method::DELETE
                && request_context
                    .path()
                    .starts_with("/api/admin/gemini-files/mappings/") =>
        {
            if !state.has_gemini_file_mapping_data_writer() {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ADMIN_GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
                )));
            }
            let Some(mapping_id) = admin_gemini_file_mapping_id_from_path(request_context.path())
            else {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::NOT_FOUND,
                    "Mapping not found",
                )));
            };
            let Some(mapping) = state.delete_gemini_file_mapping_by_id(&mapping_id).await? else {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::NOT_FOUND,
                    "Mapping not found",
                )));
            };
            Ok(Some(
                Json(json!({
                    "message": "Mapping deleted successfully",
                    "file_name": mapping.file_name,
                }))
                .into_response(),
            ))
        }
        Some("cleanup_mappings")
            if request_context.method() == http::Method::DELETE
                && is_admin_gemini_files_mappings_root(request_context.path()) =>
        {
            if !state.has_gemini_file_mapping_data_writer() {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ADMIN_GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
                )));
            }
            let deleted_count = state
                .delete_expired_gemini_file_mappings(now_unix_secs)
                .await?;
            Ok(Some(
                Json(json!({
                    "message": format!("Cleaned up {deleted_count} expired mappings"),
                    "deleted_count": deleted_count,
                }))
                .into_response(),
            ))
        }
        Some("capable_keys")
            if request_context.method() == http::Method::GET
                && is_admin_gemini_files_capable_keys_root(request_context.path()) =>
        {
            let capable_keys = admin_gemini_files_capable_keys(state).await?;
            Ok(Some(Json(capable_keys).into_response()))
        }
        _ => Ok(None),
    }
}
