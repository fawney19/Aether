use std::net::{IpAddr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_contracts::ExecutionResult;
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, VideoTaskQueryFilter, VideoTaskStatus,
};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::response::Redirect;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

mod cancel;

use super::query::VideoTaskVideoSource;
use super::{
    read_video_task_detail, read_video_task_page, read_video_task_stats,
    read_video_task_video_source,
};
use crate::{AppState, GatewayError};

pub(crate) use self::cancel::{
    cancel_video_task_record, cancel_video_task_record_for_user, CancelVideoTaskError,
};

#[derive(Debug, Deserialize)]
pub(crate) struct ListVideoTasksQuery {
    pub(crate) status: Option<String>,
    pub(crate) user_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) client_api_format: Option<String>,
    pub(crate) page: Option<usize>,
    pub(crate) page_size: Option<usize>,
}

pub(crate) async fn list_video_tasks(
    State(state): State<AppState>,
    Query(query): Query<ListVideoTasksQuery>,
) -> Result<Json<super::query::VideoTaskPageResponse>, axum::response::Response> {
    let filter = parse_filter(&query)?;
    let response = read_video_task_page(
        &state,
        &filter,
        query.page.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    .map_err(IntoResponse::into_response)?;
    Ok(Json(response))
}

pub(crate) async fn get_video_task_stats(
    State(state): State<AppState>,
    Query(query): Query<ListVideoTasksQuery>,
) -> Result<Json<super::query::VideoTaskStatsResponse>, axum::response::Response> {
    let filter = parse_filter(&query)?;
    let response = read_video_task_stats(&state, &filter, current_unix_secs())
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(Json(response))
}

pub(crate) async fn get_video_task_detail(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<StoredVideoTask>, axum::response::Response> {
    let task = read_video_task_detail(&state, &task_id)
        .await
        .map_err(IntoResponse::into_response)?;

    match task {
        Some(task) => Ok(Json(task)),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "Video task not found",
                }
            })),
        )
            .into_response()),
    }
}

pub(crate) async fn cancel_video_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, axum::response::Response> {
    let stored = cancel_video_task_record(&state, &task_id)
        .await
        .map_err(|err| match err {
            CancelVideoTaskError::NotFound => (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({
                    "error": {
                        "message": "Video task not found",
                    }
                })),
            )
                .into_response(),
            CancelVideoTaskError::InvalidStatus(status) => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "message": format!(
                            "Cannot cancel task with status: {}",
                            video_task_status_name(status),
                        ),
                    }
                })),
            )
                .into_response(),
            CancelVideoTaskError::Response(response) => response,
            CancelVideoTaskError::Gateway(err) => err.into_response(),
        })?;

    Ok(Json(json!({
        "id": stored.id,
        "status": "cancelled",
        "message": "Task cancelled successfully",
    })))
}

pub(crate) async fn get_video_task_video(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<axum::response::Response, axum::response::Response> {
    let Some(source) = read_video_task_video_source(&state, &task_id)
        .await
        .map_err(IntoResponse::into_response)?
    else {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "Video task or video not found",
                }
            })),
        )
            .into_response());
    };

    build_video_task_video_response(&state, &task_id, source)
        .await
        .map_err(IntoResponse::into_response)
}

pub(crate) async fn build_video_task_video_response(
    _state: &AppState,
    task_id: &str,
    source: VideoTaskVideoSource,
) -> Result<axum::response::Response, GatewayError> {
    match source {
        VideoTaskVideoSource::Redirect { url } => {
            resolve_public_video_target(&url).await?;
            Ok(Redirect::temporary(url.as_str()).into_response())
        }
        VideoTaskVideoSource::Proxy {
            url,
            header_name,
            header_value,
            filename,
        } => proxy_video_stream(task_id, &url, &header_name, &header_value, &filename).await,
    }
}

fn parse_filter(
    query: &ListVideoTasksQuery,
) -> Result<VideoTaskQueryFilter, axum::response::Response> {
    let status = match query.status.as_deref() {
        Some(value) => Some(VideoTaskStatus::from_database(value).map_err(|err| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "message": err.to_string(),
                    }
                })),
            )
                .into_response()
        })?),
        None => None,
    };

    Ok(VideoTaskQueryFilter {
        user_id: query.user_id.clone(),
        status,
        model_substring: query.model.clone(),
        client_api_format: query.client_api_format.clone(),
    })
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn video_task_status_name(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending => "pending",
        VideoTaskStatus::Submitted => "submitted",
        VideoTaskStatus::Queued => "queued",
        VideoTaskStatus::Processing => "processing",
        VideoTaskStatus::Completed => "completed",
        VideoTaskStatus::Failed => "failed",
        VideoTaskStatus::Cancelled => "cancelled",
        VideoTaskStatus::Expired => "expired",
        VideoTaskStatus::Deleted => "deleted",
    }
}

async fn proxy_video_stream(
    task_id: &str,
    url: &url::Url,
    header_name: &str,
    header_value: &str,
    filename: &str,
) -> Result<axum::response::Response, GatewayError> {
    let target = resolve_public_video_target(url).await?;
    let client = build_pinned_video_client(&target)?;
    let response = client
        .get(url.clone())
        .header(header_name, header_value)
        .send()
        .await
        .map_err(|err| GatewayError::UpstreamUnavailable {
            trace_id: task_id.to_string(),
            message: video_request_failure_message(&err).to_string(),
        })?;

    if response.status().is_redirection() {
        return Err(GatewayError::UpstreamUnavailable {
            trace_id: task_id.to_string(),
            message: format!(
                "video upstream redirect was rejected with HTTP {}",
                response.status()
            ),
        });
    }
    if !response.status().is_success() {
        return Err(GatewayError::UpstreamUnavailable {
            trace_id: task_id.to_string(),
            message: format!("video upstream returned HTTP {}", response.status()),
        });
    }

    let status = response.status();
    // Do not copy the provider's Content-Length onto a newly wrapped stream.
    // Reqwest may decode transfer/content encodings and the provider controls
    // the declaration; forwarding a stale value would make the client-facing
    // HTTP framing disagree with the bytes produced by this Body. Axum/Hyper
    // will select safe framing for the actual stream.
    let upstream_headers = response.headers().clone();
    let body = Body::from_stream(response.bytes_stream());

    let mut outbound = axum::http::Response::builder()
        .status(status)
        .body(body)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    apply_safe_video_response_metadata(outbound.headers_mut(), &upstream_headers, filename)?;
    Ok(outbound)
}

fn apply_safe_video_response_metadata(
    outbound: &mut axum::http::HeaderMap,
    upstream: &axum::http::HeaderMap,
    filename: &str,
) -> Result<(), GatewayError> {
    let content_type = upstream
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(safe_video_content_type)
        .unwrap_or_else(|| axum::http::HeaderValue::from_static("application/octet-stream"));
    outbound.insert(axum::http::header::CONTENT_TYPE, content_type);
    outbound.insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&format!(
            "inline; filename=\"{}\"",
            safe_video_filename(filename)
        ))
        .map_err(|err| GatewayError::Internal(err.to_string()))?,
    );
    outbound.remove(axum::http::header::CONTENT_LENGTH);
    outbound.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    outbound.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    Ok(())
}

fn safe_video_content_type(raw_value: &str) -> Option<axum::http::HeaderValue> {
    let media_type = raw_value.split(';').next()?.trim().to_ascii_lowercase();
    let subtype = media_type.strip_prefix("video/")?;
    if subtype.is_empty()
        || !subtype.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'-' | b'^' | b'_' | b'.' | b'+'
                )
        })
    {
        return None;
    }
    axum::http::HeaderValue::from_str(raw_value).ok()
}

fn safe_video_filename(filename: &str) -> String {
    let filename = filename
        .chars()
        .take(255)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if filename.is_empty() {
        "video.mp4".to_string()
    } else {
        filename
    }
}

struct ResolvedVideoTarget {
    host: String,
    addrs: Vec<SocketAddr>,
}

async fn resolve_public_video_target(url: &url::Url) -> Result<ResolvedVideoTarget, GatewayError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(video_target_rejected(
            "video URL must be an absolute HTTP(S) URL without credentials",
        ));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| video_target_rejected("video URL is missing a port"))?;
    let (host, addrs) = match url.host() {
        Some(url::Host::Ipv4(ip)) => (ip.to_string(), vec![SocketAddr::new(IpAddr::V4(ip), port)]),
        Some(url::Host::Ipv6(ip)) => (ip.to_string(), vec![SocketAddr::new(IpAddr::V6(ip), port)]),
        Some(url::Host::Domain(host)) if !host.is_empty() => {
            let addrs = aether_http::lookup_host_with_limits(
                host,
                port,
                aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
            )
            .await
            .map_err(|_| video_target_rejected("video URL DNS resolution failed"))?;
            (host.to_string(), addrs)
        }
        _ => return Err(video_target_rejected("video URL is missing a host")),
    };
    if addrs.is_empty()
        || addrs
            .iter()
            .any(|addr| aether_http::is_private_or_reserved_ip(addr.ip()))
    {
        return Err(video_target_rejected(
            "video URL resolves to a private or reserved address",
        ));
    }
    Ok(ResolvedVideoTarget { host, addrs })
}

fn build_pinned_video_client(
    target: &ResolvedVideoTarget,
) -> Result<reqwest::Client, GatewayError> {
    let mut builder = aether_http::apply_http_client_config(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none()),
        &aether_http::HttpClientConfig {
            connect_timeout_ms: Some(10_000),
            request_timeout_ms: Some(300_000),
            http2_adaptive_window: true,
            ..aether_http::HttpClientConfig::default()
        },
    );
    if target.host.parse::<IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&target.host, &target.addrs);
    }
    builder
        .build()
        .map_err(|_| GatewayError::Internal("video HTTP client initialization failed".to_string()))
}

fn video_target_rejected(message: &str) -> GatewayError {
    GatewayError::Client {
        status: axum::http::StatusCode::BAD_GATEWAY,
        message: message.to_string(),
    }
}

fn video_request_failure_message(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "video upstream request timed out"
    } else if error.is_connect() {
        "video upstream connection failed"
    } else if error.is_body() || error.is_decode() {
        "video upstream response failed"
    } else {
        "video upstream request failed"
    }
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    use super::{
        apply_safe_video_response_metadata, build_video_task_video_response,
        resolve_public_video_target, safe_video_content_type, safe_video_filename,
        VideoTaskVideoSource,
    };
    use crate::AppState;

    #[tokio::test]
    async fn video_redirect_response_accepts_public_target() {
        let state = AppState::new().expect("gateway state should build");
        let target = "https://8.8.8.8/video.mp4";

        let response = build_video_task_video_response(
            &state,
            "task-public-redirect",
            VideoTaskVideoSource::Redirect {
                url: url::Url::parse(target).expect("public target should parse"),
            },
        )
        .await
        .expect("public redirect should build");

        assert_eq!(
            response.status(),
            axum::http::StatusCode::TEMPORARY_REDIRECT
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(target)
        );
    }

    #[tokio::test]
    async fn video_redirect_response_rejects_private_and_reserved_targets() {
        let state = AppState::new().expect("gateway state should build");

        for raw_url in [
            "http://127.0.0.1/video.mp4",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/video.mp4",
            "http://[::1]/video.mp4",
        ] {
            let error = build_video_task_video_response(
                &state,
                "task-rejected-redirect",
                VideoTaskVideoSource::Redirect {
                    url: url::Url::parse(raw_url).expect("target should parse"),
                },
            )
            .await
            .expect_err("private or reserved redirect target should be rejected");

            assert_eq!(
                error.into_response().status(),
                axum::http::StatusCode::BAD_GATEWAY,
                "unexpected status for {raw_url}"
            );
        }
    }

    #[tokio::test]
    async fn video_target_resolution_rejects_private_and_reserved_ip_literals() {
        for raw_url in [
            "http://127.0.0.1/video.mp4",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/video.mp4",
            "http://[::1]/video.mp4",
            "http://[::ffff:127.0.0.1]/video.mp4",
        ] {
            let url = url::Url::parse(raw_url).unwrap();
            assert!(
                resolve_public_video_target(&url).await.is_err(),
                "target should be rejected: {raw_url}"
            );
        }
    }

    #[tokio::test]
    async fn video_target_resolution_accepts_public_ip_literals() {
        for raw_url in [
            "https://8.8.8.8/video.mp4",
            "https://[2606:4700:4700::1111]/video.mp4",
        ] {
            let url = url::Url::parse(raw_url).unwrap();
            assert!(
                resolve_public_video_target(&url).await.is_ok(),
                "target should be accepted: {raw_url}"
            );
        }
    }

    #[test]
    fn video_response_metadata_rejects_active_content_and_sanitizes_filename() {
        assert!(safe_video_content_type("video/mp4").is_some());
        assert!(safe_video_content_type("video/webm; charset=binary").is_some());
        assert!(safe_video_content_type("video/").is_none());
        assert!(safe_video_content_type("video/; charset=binary").is_none());
        assert!(safe_video_content_type("text/html").is_none());
        assert!(safe_video_content_type("video/mp4\r\nx-test: injected").is_none());
        assert_eq!(
            safe_video_filename("video_123.mp4\"; filename=\"attack.html"),
            "video_123.mp4___filename__attack.html"
        );
        assert_eq!(safe_video_filename(&"x".repeat(1024)).len(), 255);

        let mut upstream = axum::http::HeaderMap::new();
        upstream.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/html"),
        );
        upstream.insert(
            axum::http::header::CONTENT_LENGTH,
            axum::http::HeaderValue::from_static("999999"),
        );
        let mut outbound = upstream.clone();
        apply_safe_video_response_metadata(
            &mut outbound,
            &upstream,
            "video.mp4\"; filename=\"attack.html",
        )
        .expect("video metadata should build");

        assert_eq!(
            outbound
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/octet-stream")
        );
        assert!(outbound.get(axum::http::header::CONTENT_LENGTH).is_none());
        assert_eq!(
            outbound
                .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            outbound
                .get(axum::http::header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            Some("inline; filename=\"video.mp4___filename__attack.html\"")
        );
    }
}
