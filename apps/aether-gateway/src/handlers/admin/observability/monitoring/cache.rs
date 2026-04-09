use super::cache_config::{
    ADMIN_MONITORING_CACHE_AFFINITY_DEFAULT_TTL_SECS, ADMIN_MONITORING_CACHE_RESERVATION_RATIO,
    ADMIN_MONITORING_DYNAMIC_RESERVATION_HIGH_LOAD_THRESHOLD,
    ADMIN_MONITORING_DYNAMIC_RESERVATION_LOW_LOAD_THRESHOLD,
    ADMIN_MONITORING_DYNAMIC_RESERVATION_PROBE_PHASE_REQUESTS,
    ADMIN_MONITORING_DYNAMIC_RESERVATION_PROBE_RESERVATION,
    ADMIN_MONITORING_DYNAMIC_RESERVATION_STABLE_MAX_RESERVATION,
    ADMIN_MONITORING_DYNAMIC_RESERVATION_STABLE_MIN_RESERVATION,
};
use super::cache_store::build_admin_monitoring_cache_snapshot;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_monitoring_cache_stats_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let snapshot = build_admin_monitoring_cache_snapshot(state).await?;

    Ok(Json(json!({
        "status": "ok",
        "data": {
            "scheduler": snapshot.scheduler_name,
            "total_affinities": snapshot.total_affinities,
            "cache_hit_rate": snapshot.cache_hit_rate,
            "provider_switches": snapshot.provider_switches,
            "key_switches": snapshot.key_switches,
            "cache_hits": snapshot.cache_hits,
            "cache_misses": snapshot.cache_misses,
            "scheduler_metrics": {
                "cache_hits": snapshot.cache_hits,
                "cache_misses": snapshot.cache_misses,
                "cache_hit_rate": snapshot.cache_hit_rate,
                "total_batches": 0,
                "last_batch_size": 0,
                "total_candidates": 0,
                "last_candidate_count": 0,
                "concurrency_denied": 0,
                "avg_candidates_per_batch": 0.0,
                "scheduling_mode": snapshot.scheduling_mode,
                "provider_priority_mode": snapshot.provider_priority_mode,
            },
            "affinity_stats": {
                "storage_type": snapshot.storage_type,
                "total_affinities": snapshot.total_affinities,
                "cache_hits": snapshot.cache_hits,
                "cache_misses": snapshot.cache_misses,
                "cache_hit_rate": snapshot.cache_hit_rate,
                "cache_invalidations": snapshot.cache_invalidations,
                "provider_switches": snapshot.provider_switches,
                "key_switches": snapshot.key_switches,
                "config": {
                    "default_ttl": ADMIN_MONITORING_CACHE_AFFINITY_DEFAULT_TTL_SECS,
                }
            }
        }
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_cache_metrics_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let snapshot = build_admin_monitoring_cache_snapshot(state).await?;
    let metrics = [
        (
            "cache_scheduler_total_batches",
            "Number of scheduling batches processed",
            0.0,
        ),
        (
            "cache_scheduler_last_batch_size",
            "Size of the most recent scheduling batch",
            0.0,
        ),
        (
            "cache_scheduler_total_candidates",
            "Total candidates seen during scheduling",
            0.0,
        ),
        (
            "cache_scheduler_last_candidate_count",
            "Number of candidates in the most recent batch",
            0.0,
        ),
        (
            "cache_scheduler_cache_hits",
            "Cache hits counted during scheduling",
            snapshot.cache_hits as f64,
        ),
        (
            "cache_scheduler_cache_misses",
            "Cache misses counted during scheduling",
            snapshot.cache_misses as f64,
        ),
        (
            "cache_scheduler_cache_hit_rate",
            "Cache hit rate during scheduling",
            snapshot.cache_hit_rate,
        ),
        (
            "cache_scheduler_concurrency_denied",
            "Times candidate rejected due to concurrency limits",
            0.0,
        ),
        (
            "cache_scheduler_avg_candidates_per_batch",
            "Average candidates per batch",
            0.0,
        ),
        (
            "cache_affinity_total",
            "Total cache affinities stored",
            snapshot.total_affinities as f64,
        ),
        (
            "cache_affinity_hits",
            "Affinity cache hits",
            snapshot.cache_hits as f64,
        ),
        (
            "cache_affinity_misses",
            "Affinity cache misses",
            snapshot.cache_misses as f64,
        ),
        (
            "cache_affinity_hit_rate",
            "Affinity cache hit rate",
            snapshot.cache_hit_rate,
        ),
        (
            "cache_affinity_invalidations",
            "Affinity invalidations",
            snapshot.cache_invalidations as f64,
        ),
        (
            "cache_affinity_provider_switches",
            "Affinity provider switches",
            snapshot.provider_switches as f64,
        ),
        (
            "cache_affinity_key_switches",
            "Affinity key switches",
            snapshot.key_switches as f64,
        ),
    ];

    let mut lines = Vec::with_capacity(metrics.len() * 3 + 1);
    for (name, help_text, value) in metrics {
        lines.push(format!("# HELP {name} {help_text}"));
        lines.push(format!("# TYPE {name} gauge"));
        lines.push(format!("{name} {value}"));
    }
    lines.push(format!(
        "cache_scheduler_info{{scheduler=\"{}\"}} 1",
        snapshot.scheduler_name
    ));

    Ok((
        [(
            http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        lines.join("\n") + "\n",
    )
        .into_response())
}

pub(super) async fn build_admin_monitoring_cache_config_response(
) -> Result<Response<Body>, GatewayError> {
    Ok(Json(json!({
        "status": "ok",
        "data": {
            "cache_ttl_seconds": ADMIN_MONITORING_CACHE_AFFINITY_DEFAULT_TTL_SECS,
            "cache_reservation_ratio": ADMIN_MONITORING_CACHE_RESERVATION_RATIO,
            "dynamic_reservation": {
                "enabled": true,
                "config": {
                    "probe_phase_requests": ADMIN_MONITORING_DYNAMIC_RESERVATION_PROBE_PHASE_REQUESTS,
                    "probe_reservation": ADMIN_MONITORING_DYNAMIC_RESERVATION_PROBE_RESERVATION,
                    "stable_min_reservation": ADMIN_MONITORING_DYNAMIC_RESERVATION_STABLE_MIN_RESERVATION,
                    "stable_max_reservation": ADMIN_MONITORING_DYNAMIC_RESERVATION_STABLE_MAX_RESERVATION,
                    "low_load_threshold": ADMIN_MONITORING_DYNAMIC_RESERVATION_LOW_LOAD_THRESHOLD,
                    "high_load_threshold": ADMIN_MONITORING_DYNAMIC_RESERVATION_HIGH_LOAD_THRESHOLD,
                },
                "description": {
                    "probe_phase_requests": "探测阶段请求数阈值",
                    "probe_reservation": "探测阶段预留比例",
                    "stable_min_reservation": "稳定阶段最小预留比例",
                    "stable_max_reservation": "稳定阶段最大预留比例",
                    "low_load_threshold": "低负载阈值（低于此值使用最小预留）",
                    "high_load_threshold": "高负载阈值（高于此值根据置信度使用较高预留）",
                },
            },
            "description": {
                "cache_ttl": "缓存亲和性有效期（秒）",
                "cache_reservation_ratio": "静态预留比例（已被动态预留替代）",
                "dynamic_reservation": "动态预留机制配置",
            },
        }
    }))
    .into_response())
}
