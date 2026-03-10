"""
Prometheus metrics for monitoring
"""

from prometheus_client import Counter, Gauge, Histogram

# 并发槽位占用时长分布（按异常类型聚合，不按 key_id 拆分）
concurrency_slot_duration_seconds = Histogram(
    "concurrency_slot_duration_seconds",
    "Duration of concurrency slot occupation in seconds",
    ["exception"],
    buckets=[0.1, 0.5, 1, 5, 10, 30, 60, 120, 300, 600],  # 0.1s 到 10 分钟
)

# 并发槽位释放计数
concurrency_slot_release_total = Counter(
    "concurrency_slot_release_total",
    "Total number of concurrency slot releases",
    ["exception"],
)

# 请求总数（按类型）
request_total = Counter(
    "request_total",
    "Total number of requests",
    ["type", "status"],  # type values: streaming/non-streaming, status: success/error
)

# 调度：并发拒绝计数（RPM guard）
scheduler_concurrency_denied_total = Counter(
    "scheduler_concurrency_denied_total",
    "Total number of candidates skipped due to concurrency/RPM limits",
    ["is_cached_user", "reason", "reservation_phase"],
)

# 健康监控相关
health_open_circuits = Gauge(
    "health_open_circuits",
    "Number of provider keys currently in circuit breaker open state",
)

# 模型映射解析相关
model_mapping_resolution_total = Counter(
    "model_mapping_resolution_total",
    "Total number of model mapping resolutions",
    ["method", "cache_hit"],
    # method: direct_match, provider_model_name, mapping, not_found
    # cache_hit: true, false
)

model_mapping_resolution_duration_seconds = Histogram(
    "model_mapping_resolution_duration_seconds",
    "Duration of model mapping resolution in seconds",
    ["method"],
    buckets=[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0],  # 1ms 到 1s
)

model_mapping_conflict_total = Counter(
    "model_mapping_conflict_total",
    "Total number of mapping conflicts detected (same name maps to multiple GlobalModels)",
)

# ==================== API 格式转换 ====================

format_conversion_total = Counter(
    "format_conversion_total",
    "Total number of format conversions",
    ["direction", "source_format", "target_format", "status"],  # status: success/error
)

format_conversion_duration_seconds = Histogram(
    "format_conversion_duration_seconds",
    "Duration of format conversions in seconds",
    ["direction", "source_format", "target_format"],
    buckets=[0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0],
)

# ==================== Billing migration / shadow billing ====================

billing_requests_total = Counter(
    "billing_requests_total",
    "Total number of billing calculations",
    ["engine_mode", "truth_engine"],  # low-cardinality labels
)

billing_fallback_total = Counter(
    "billing_fallback_total",
    "Total number of billing fallbacks to legacy engine",
)

billing_diff_exceeds_threshold_total = Counter(
    "billing_diff_exceeds_threshold_total",
    "Total number of shadow billing diffs exceeding threshold",
    ["engine_mode"],
)

billing_invariant_violation_total = Counter(
    "billing_invariant_violation_total",
    "Total number of billing invariant violations (sum(breakdown)!=total)",
    ["engine_mode", "truth_engine"],
)

# ==================== Antigravity ====================

antigravity_degradation_total = Counter(
    "aether_antigravity_degradation_total",
    "Count of Antigravity signature degradation (rectification) events",
    ["stage"],
)
