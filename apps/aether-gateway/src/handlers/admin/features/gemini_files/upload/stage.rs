use super::super::admin_gemini_files_key_capable;
use super::request::AdminGeminiFilesUploadRequest;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_contracts::{ExecutionPlan, ExecutionResult, RequestBody};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use axum::http;
use base64::Engine;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
struct AdminGeminiFilesUploadExecutionSuccess {
    file_name: String,
    display_name: Option<String>,
    mime_type: Option<String>,
}

pub(super) async fn admin_gemini_files_upload_across_keys(
    state: &AdminAppState<'_>,
    execution_runtime_base_url: &str,
    trace_id: &str,
    upload: &AdminGeminiFilesUploadRequest,
    requested_key_ids: &[String],
) -> Result<serde_json::Value, GatewayError> {
    let keys = state
        .read_provider_catalog_keys_by_ids(requested_key_ids)
        .await?;
    let key_by_id = keys
        .iter()
        .map(|key| (key.id.as_str(), key))
        .collect::<BTreeMap<_, _>>();
    let provider_ids = keys
        .iter()
        .map(|key| key.provider_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?;
    let endpoints_by_provider_id = endpoints.into_iter().fold(
        BTreeMap::<String, Vec<StoredProviderCatalogEndpoint>>::new(),
        |mut out, endpoint| {
            out.entry(endpoint.provider_id.clone())
                .or_default()
                .push(endpoint);
            out
        },
    );

    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut fail_count = 0usize;

    for key_id in requested_key_ids {
        let Some(key) = key_by_id.get(key_id.as_str()) else {
            fail_count += 1;
            results.push(json!({
                "key_id": key_id,
                "key_name": serde_json::Value::Null,
                "success": false,
                "file_name": serde_json::Value::Null,
                "error": "Key 不存在",
            }));
            continue;
        };

        let key_name = Some(key.name.clone());
        let outcome = admin_gemini_files_upload_single_key(
            state,
            execution_runtime_base_url,
            trace_id,
            upload,
            key,
            endpoints_by_provider_id.get(&key.provider_id),
        )
        .await;

        match outcome {
            Ok(success) => {
                success_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key_name,
                    "success": true,
                    "file_name": success.file_name,
                    "error": serde_json::Value::Null,
                }));
            }
            Err(error) => {
                fail_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key_name,
                    "success": false,
                    "file_name": serde_json::Value::Null,
                    "error": error,
                }));
            }
        }
    }

    Ok(json!({
        "display_name": upload.display_name,
        "mime_type": upload.mime_type,
        "size_bytes": upload.body_bytes.len(),
        "results": results,
        "success_count": success_count,
        "fail_count": fail_count,
    }))
}

async fn admin_gemini_files_upload_single_key(
    state: &AdminAppState<'_>,
    _execution_runtime_base_url: &str,
    trace_id: &str,
    upload: &AdminGeminiFilesUploadRequest,
    key: &StoredProviderCatalogKey,
    endpoints: Option<&Vec<StoredProviderCatalogEndpoint>>,
) -> Result<AdminGeminiFilesUploadExecutionSuccess, String> {
    if !admin_gemini_files_key_capable(key) {
        return Err("Key 不支持 Gemini Files".to_string());
    }
    let Some(endpoint) = endpoints.and_then(|endpoints| {
        endpoints.iter().find(|endpoint| {
            endpoint.is_active
                && endpoint
                    .api_format
                    .trim()
                    .eq_ignore_ascii_case("gemini:chat")
        })
    }) else {
        return Err("找不到有效的 gemini:chat 端点".to_string());
    };
    let transport = state
        .read_provider_transport_snapshot(&key.provider_id, &endpoint.id, &key.id)
        .await
        .map_err(|err| format!("{err:?}"))?
        .ok_or_else(|| "无法读取 Key 传输配置".to_string())?;
    if !state.supports_local_gemini_transport_with_network(&transport, "gemini:chat") {
        return Err("Key 传输配置不支持 Gemini Files 上传".to_string());
    }
    if transport.endpoint.body_rules.is_some() {
        return Err("Gemini Files 二进制上传暂不支持 endpoint body_rules".to_string());
    }
    let (auth_header, auth_value) = state
        .resolve_local_gemini_auth(&transport)
        .ok_or_else(|| "Key 缺少可用的 Gemini 认证信息".to_string())?;

    let mut provider_request_headers = state.build_passthrough_headers_with_auth(
        &http::HeaderMap::new(),
        &auth_header,
        &auth_value,
        &BTreeMap::new(),
    );
    provider_request_headers.insert("content-type".to_string(), upload.mime_type.clone());
    let original_request_body = json!({
        "body_bytes_b64": upload.body_bytes_b64,
    });
    if !state.apply_local_header_rules(
        &mut provider_request_headers,
        transport.endpoint.header_rules.as_ref(),
        &[auth_header.as_str(), "content-type"],
        &original_request_body,
        Some(&original_request_body),
    ) {
        return Err("Key 端点 header_rules 应用失败".to_string());
    }

    let upload_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/upload/v1beta/files");
    let upload_query = if upload_path.contains("uploadType=") {
        None
    } else {
        Some("uploadType=resumable")
    };
    let upstream_url = state
        .build_gemini_files_passthrough_url(&transport.endpoint.base_url, upload_path, upload_query)
        .ok_or_else(|| "无法构建 Gemini Files 上传地址".to_string())?;

    let plan = ExecutionPlan {
        request_id: format!("{trace_id}:admin-gemini-upload:{}", key.id),
        candidate_id: None,
        provider_name: Some(transport.provider.name.clone()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "POST".to_string(),
        url: upstream_url,
        headers: provider_request_headers,
        content_type: Some(upload.mime_type.clone()),
        content_encoding: None,
        body: RequestBody {
            json_body: None,
            body_bytes_b64: Some(upload.body_bytes_b64.clone()),
            body_ref: None,
        },
        stream: false,
        client_api_format: "gemini:files".to_string(),
        provider_api_format: "gemini:files".to_string(),
        model_name: Some("gemini-files".to_string()),
        proxy: state
            .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
            .await,
        tls_profile: state.resolve_transport_tls_profile(&transport),
        timeouts: state.resolve_transport_execution_timeouts(&transport),
    };

    let result = admin_gemini_files_execute_upload_plan(state, trace_id, &plan)
        .await
        .map_err(|error| format!("{error:?}"))?;
    if result.status_code >= 400 {
        return Err(admin_gemini_files_execution_error_message(&result));
    }
    let body_json = admin_gemini_files_execution_json_body(&result)
        .ok_or_else(|| "上传成功但上游响应缺少 JSON body".to_string())?;
    let success = admin_gemini_files_upload_success_from_body(&body_json, upload)
        .ok_or_else(|| admin_gemini_files_execution_error_message(&result))?;
    state
        .store_local_gemini_file_mapping(
            success.file_name.as_str(),
            key.id.as_str(),
            None,
            success
                .display_name
                .as_deref()
                .or(Some(upload.display_name.as_str())),
            success
                .mime_type
                .as_deref()
                .or(Some(upload.mime_type.as_str())),
        )
        .await
        .map_err(|err| format!("上传成功但本地映射写入失败: {err:?}"))?;
    Ok(success)
}

async fn admin_gemini_files_execute_upload_plan(
    state: &AdminAppState<'_>,
    trace_id: &str,
    plan: &ExecutionPlan,
) -> Result<ExecutionResult, GatewayError> {
    state
        .execute_execution_runtime_sync_plan(Some(trace_id), plan)
        .await
}

fn admin_gemini_files_execution_json_body(result: &ExecutionResult) -> Option<serde_json::Value> {
    if let Some(body_json) = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
    {
        return Some(body_json.clone());
    }
    let content_type = result
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_ascii_lowercase());
    if !content_type
        .as_deref()
        .is_some_and(|value| value.starts_with("application/json"))
    {
        return None;
    }
    let body_bytes_b64 = result
        .body
        .as_ref()
        .and_then(|body| body.body_bytes_b64.as_deref())?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(body_bytes_b64)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn admin_gemini_files_upload_success_from_body(
    body_json: &serde_json::Value,
    upload: &AdminGeminiFilesUploadRequest,
) -> Option<AdminGeminiFilesUploadExecutionSuccess> {
    let file_object = body_json
        .get("file")
        .and_then(serde_json::Value::as_object)
        .or_else(|| body_json.as_object())?;
    let file_name = file_object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let display_name = file_object
        .get("displayName")
        .or_else(|| file_object.get("display_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| Some(upload.display_name.clone()));
    let mime_type = file_object
        .get("mimeType")
        .or_else(|| file_object.get("mime_type"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| Some(upload.mime_type.clone()));
    Some(AdminGeminiFilesUploadExecutionSuccess {
        file_name: file_name.to_string(),
        display_name,
        mime_type,
    })
}

fn admin_gemini_files_execution_error_message(result: &ExecutionResult) -> String {
    if let Some(body_json) = admin_gemini_files_execution_json_body(result) {
        if let Some(message) = body_json
            .get("error")
            .and_then(serde_json::Value::as_object)
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return message.to_string();
        }
        if let Some(message) = body_json
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return message.to_string();
        }
    }
    if let Some(error) = result
        .error
        .as_ref()
        .map(|error| error.message.trim())
        .filter(|value| !value.is_empty())
    {
        return error.to_string();
    }
    format!("上传失败，状态码 {}", result.status_code)
}
