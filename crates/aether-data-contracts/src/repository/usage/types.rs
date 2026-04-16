use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredRequestUsageAudit {
    pub id: String,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub provider_name: String,
    pub model: String,
    pub target_model: Option<String>,
    pub provider_id: Option<String>,
    pub provider_endpoint_id: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub request_type: Option<String>,
    pub api_format: Option<String>,
    pub api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub endpoint_api_format: Option<String>,
    pub provider_api_family: Option<String>,
    pub provider_endpoint_kind: Option<String>,
    pub has_format_conversion: bool,
    pub is_stream: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_creation_ephemeral_5m_input_tokens: u64,
    pub cache_creation_ephemeral_1h_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
    pub output_price_per_1m: Option<f64>,
    pub total_cost_usd: f64,
    pub actual_total_cost_usd: f64,
    pub status_code: Option<u16>,
    pub error_message: Option<String>,
    pub error_category: Option<String>,
    pub response_time_ms: Option<u64>,
    pub first_byte_time_ms: Option<u64>,
    pub status: String,
    pub billing_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_execution_runtime_miss_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_metadata: Option<Value>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_secs: u64,
    pub finalized_at_unix_secs: Option<u64>,
}

impl StoredRequestUsageAudit {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        request_id: String,
        user_id: Option<String>,
        api_key_id: Option<String>,
        username: Option<String>,
        api_key_name: Option<String>,
        provider_name: String,
        model: String,
        target_model: Option<String>,
        provider_id: Option<String>,
        provider_endpoint_id: Option<String>,
        provider_api_key_id: Option<String>,
        request_type: Option<String>,
        api_format: Option<String>,
        api_family: Option<String>,
        endpoint_kind: Option<String>,
        endpoint_api_format: Option<String>,
        provider_api_family: Option<String>,
        provider_endpoint_kind: Option<String>,
        has_format_conversion: bool,
        is_stream: bool,
        input_tokens: i32,
        output_tokens: i32,
        total_tokens: i32,
        total_cost_usd: f64,
        actual_total_cost_usd: f64,
        status_code: Option<i32>,
        error_message: Option<String>,
        error_category: Option<String>,
        response_time_ms: Option<i32>,
        first_byte_time_ms: Option<i32>,
        status: String,
        billing_status: String,
        created_at_unix_ms: i64,
        updated_at_unix_secs: i64,
        finalized_at_unix_secs: Option<i64>,
    ) -> Result<Self, crate::DataLayerError> {
        if request_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "usage.request_id is empty".to_string(),
            ));
        }
        if provider_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "usage.provider_name is empty".to_string(),
            ));
        }
        if model.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "usage.model is empty".to_string(),
            ));
        }
        if status.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "usage.status is empty".to_string(),
            ));
        }
        if billing_status.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "usage.billing_status is empty".to_string(),
            ));
        }
        if !total_cost_usd.is_finite() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "usage.total_cost_usd is not finite".to_string(),
            ));
        }
        if !actual_total_cost_usd.is_finite() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "usage.actual_total_cost_usd is not finite".to_string(),
            ));
        }

        Ok(Self {
            id,
            request_id,
            user_id,
            api_key_id,
            username,
            api_key_name,
            provider_name,
            model,
            target_model,
            provider_id,
            provider_endpoint_id,
            provider_api_key_id,
            request_type,
            api_format,
            api_family,
            endpoint_kind,
            endpoint_api_format,
            provider_api_family,
            provider_endpoint_kind,
            has_format_conversion,
            is_stream,
            input_tokens: parse_u64(input_tokens, "usage.input_tokens")?,
            output_tokens: parse_u64(output_tokens, "usage.output_tokens")?,
            total_tokens: parse_u64(total_tokens, "usage.total_tokens")?,
            cache_creation_input_tokens: 0,
            cache_creation_ephemeral_5m_input_tokens: 0,
            cache_creation_ephemeral_1h_input_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_cost_usd: 0.0,
            cache_read_cost_usd: 0.0,
            output_price_per_1m: None,
            total_cost_usd,
            actual_total_cost_usd,
            status_code: parse_u16(status_code, "usage.status_code")?,
            error_message,
            error_category,
            response_time_ms: parse_optional_u64(response_time_ms, "usage.response_time_ms")?,
            first_byte_time_ms: parse_optional_u64(first_byte_time_ms, "usage.first_byte_time_ms")?,
            status,
            billing_status,
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            created_at_unix_ms: parse_timestamp(created_at_unix_ms, "usage.created_at_unix_ms")?,
            updated_at_unix_secs: parse_timestamp(
                updated_at_unix_secs,
                "usage.updated_at_unix_secs",
            )?,
            finalized_at_unix_secs: finalized_at_unix_secs
                .map(|value| parse_timestamp(value, "usage.finalized_at_unix_secs"))
                .transpose()?,
        })
    }

    pub fn with_cache_input_tokens(
        mut self,
        cache_creation_input_tokens: u64,
        cache_read_input_tokens: u64,
    ) -> Self {
        self.cache_creation_input_tokens = cache_creation_input_tokens;
        self.cache_read_input_tokens = cache_read_input_tokens;
        self
    }

    fn request_metadata_object(&self) -> Option<&serde_json::Map<String, Value>> {
        self.request_metadata.as_ref().and_then(Value::as_object)
    }

    fn request_metadata_number(&self, key: &str) -> Option<f64> {
        self.request_metadata_object()
            .and_then(|metadata| metadata.get(key))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
    }

    fn request_metadata_u64(&self, key: &str) -> Option<u64> {
        self.request_metadata_object()
            .and_then(|metadata| metadata.get(key))
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
            })
    }

    fn request_metadata_bool(&self, key: &str) -> Option<bool> {
        self.request_metadata_object()
            .and_then(|metadata| metadata.get(key))
            .and_then(Value::as_bool)
    }

    fn request_metadata_string(&self, key: &str) -> Option<&str> {
        self.request_metadata_object()
            .and_then(|metadata| metadata.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn billing_snapshot_resolved_number(&self, key: &str) -> Option<f64> {
        self.request_metadata_object()
            .and_then(|metadata| metadata.get("billing_snapshot"))
            .and_then(Value::as_object)
            .and_then(|snapshot| snapshot.get("resolved_variables"))
            .and_then(Value::as_object)
            .and_then(|variables| variables.get(key))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
    }

    pub fn settlement_billing_snapshot_schema_version(&self) -> Option<&str> {
        self.request_metadata_string("billing_snapshot_schema_version")
    }

    pub fn settlement_billing_snapshot_status(&self) -> Option<&str> {
        self.request_metadata_string("billing_snapshot_status")
    }

    pub fn settlement_rate_multiplier(&self) -> Option<f64> {
        self.request_metadata_number("rate_multiplier")
    }

    pub fn settlement_is_free_tier(&self) -> Option<bool> {
        self.request_metadata_bool("is_free_tier")
    }

    pub fn settlement_input_price_per_1m(&self) -> Option<f64> {
        self.request_metadata_number("input_price_per_1m")
            .or_else(|| self.billing_snapshot_resolved_number("input_price_per_1m"))
    }

    pub fn settlement_output_price_per_1m(&self) -> Option<f64> {
        self.request_metadata_number("output_price_per_1m")
            .or_else(|| self.billing_snapshot_resolved_number("output_price_per_1m"))
            .or(self.output_price_per_1m)
    }

    pub fn settlement_cache_creation_price_per_1m(&self) -> Option<f64> {
        self.request_metadata_number("cache_creation_price_per_1m")
            .or_else(|| self.billing_snapshot_resolved_number("cache_creation_price_per_1m"))
    }

    pub fn settlement_cache_read_price_per_1m(&self) -> Option<f64> {
        self.request_metadata_number("cache_read_price_per_1m")
            .or_else(|| self.billing_snapshot_resolved_number("cache_read_price_per_1m"))
    }

    pub fn settlement_price_per_request(&self) -> Option<f64> {
        self.request_metadata_number("price_per_request")
            .or_else(|| self.billing_snapshot_resolved_number("price_per_request"))
    }

    pub fn trace_id(&self) -> Option<&str> {
        self.request_metadata_string("trace_id")
    }

    pub fn body_ref(&self, field: UsageBodyField) -> Option<&str> {
        match field {
            UsageBodyField::RequestBody => self.request_body_ref.as_deref(),
            UsageBodyField::ProviderRequestBody => self.provider_request_body_ref.as_deref(),
            UsageBodyField::ResponseBody => self.response_body_ref.as_deref(),
            UsageBodyField::ClientResponseBody => self.client_response_body_ref.as_deref(),
        }
    }

    pub fn routing_candidate_id(&self) -> Option<&str> {
        self.candidate_id
            .as_deref()
            .or_else(|| self.request_metadata_string("candidate_id"))
    }

    pub fn routing_candidate_index(&self) -> Option<u64> {
        self.candidate_index
            .or_else(|| self.request_metadata_u64("candidate_index"))
    }

    pub fn routing_key_name(&self) -> Option<&str> {
        self.key_name
            .as_deref()
            .or_else(|| self.request_metadata_string("key_name"))
    }

    pub fn routing_planner_kind(&self) -> Option<&str> {
        self.planner_kind
            .as_deref()
            .or_else(|| self.request_metadata_string("planner_kind"))
    }

    pub fn routing_route_family(&self) -> Option<&str> {
        self.route_family
            .as_deref()
            .or_else(|| self.request_metadata_string("route_family"))
    }

    pub fn routing_route_kind(&self) -> Option<&str> {
        self.route_kind
            .as_deref()
            .or_else(|| self.request_metadata_string("route_kind"))
    }

    pub fn routing_execution_path(&self) -> Option<&str> {
        self.execution_path
            .as_deref()
            .or_else(|| self.request_metadata_string("execution_path"))
    }

    pub fn routing_local_execution_runtime_miss_reason(&self) -> Option<&str> {
        self.local_execution_runtime_miss_reason
            .as_deref()
            .or_else(|| self.request_metadata_string("local_execution_runtime_miss_reason"))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderUsageWindow {
    pub provider_id: String,
    pub window_start_unix_secs: u64,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub avg_response_time_ms: f64,
    pub total_cost_usd: f64,
}

impl StoredProviderUsageWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider_id: String,
        window_start_unix_secs: i64,
        total_requests: i64,
        successful_requests: i64,
        failed_requests: i64,
        avg_response_time_ms: f64,
        total_cost_usd: f64,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider usage window provider_id is empty".to_string(),
            ));
        }
        if !avg_response_time_ms.is_finite() || !total_cost_usd.is_finite() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider usage window value is not finite".to_string(),
            ));
        }

        Ok(Self {
            provider_id,
            window_start_unix_secs: parse_timestamp(
                window_start_unix_secs,
                "provider_usage_tracking.window_start_unix_secs",
            )?,
            total_requests: parse_timestamp(
                total_requests,
                "provider_usage_tracking.total_requests",
            )?,
            successful_requests: parse_timestamp(
                successful_requests,
                "provider_usage_tracking.successful_requests",
            )?,
            failed_requests: parse_timestamp(
                failed_requests,
                "provider_usage_tracking.failed_requests",
            )?,
            avg_response_time_ms,
            total_cost_usd,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderUsageSummary {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub avg_response_time_ms: f64,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderApiKeyUsageSummary {
    pub provider_api_key_id: String,
    pub request_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub last_used_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct UsageAuditListQuery {
    pub created_from_unix_secs: Option<u64>,
    pub created_until_unix_secs: Option<u64>,
    pub user_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
    pub api_format: Option<String>,
    pub statuses: Option<Vec<String>>,
    pub is_stream: Option<bool>,
    pub error_only: bool,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub newest_first: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageAuditAggregationGroupBy {
    Model,
    Provider,
    ApiFormat,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsageAuditAggregationQuery {
    pub created_from_unix_secs: u64,
    pub created_until_unix_secs: u64,
    pub group_by: UsageAuditAggregationGroupBy,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct UsageAuditSummaryQuery {
    pub created_from_unix_secs: u64,
    pub created_until_unix_secs: u64,
    pub user_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUsageAuditAggregation {
    pub group_key: String,
    pub display_name: Option<String>,
    pub secondary_name: Option<String>,
    pub request_count: u64,
    pub total_tokens: u64,
    pub output_tokens: u64,
    pub effective_input_tokens: u64,
    pub total_input_context: u64,
    pub cache_creation_tokens: u64,
    pub cache_creation_ephemeral_5m_tokens: u64,
    pub cache_creation_ephemeral_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost_usd: f64,
    pub actual_total_cost_usd: f64,
    pub avg_response_time_ms: Option<f64>,
    pub success_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredUsageAuditSummary {
    pub total_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub recorded_total_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_creation_ephemeral_5m_tokens: u64,
    pub cache_creation_ephemeral_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost_usd: f64,
    pub actual_total_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
    pub total_response_time_ms: f64,
    pub error_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageTimeSeriesGranularity {
    Hour,
    Day,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsageTimeSeriesQuery {
    pub created_from_unix_secs: u64,
    pub created_until_unix_secs: u64,
    pub granularity: UsageTimeSeriesGranularity,
    pub tz_offset_minutes: i32,
    pub user_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredUsageTimeSeriesBucket {
    pub bucket_key: String,
    pub total_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost_usd: f64,
    pub total_response_time_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageLeaderboardGroupBy {
    Model,
    User,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsageLeaderboardQuery {
    pub created_from_unix_secs: u64,
    pub created_until_unix_secs: u64,
    pub group_by: UsageLeaderboardGroupBy,
    pub user_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredUsageLeaderboardSummary {
    pub group_key: String,
    pub legacy_name: Option<String>,
    pub request_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct UsageDailyHeatmapQuery {
    pub created_from_unix_secs: u64,
    pub user_id: Option<String>,
    /// When true, exclude rows with status in ('pending', 'streaming') (admin heatmap).
    /// When false, only include rows with billing_status = 'settled' and total_cost_usd > 0 (user heatmap).
    pub admin_mode: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUsageDailySummary {
    /// Date as "YYYY-MM-DD"
    pub date: String,
    pub requests: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub actual_total_cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageBodyField {
    RequestBody,
    ProviderRequestBody,
    ResponseBody,
    ClientResponseBody,
}

impl UsageBodyField {
    pub fn as_ref_key(&self) -> &'static str {
        match self {
            Self::RequestBody => "request_body_ref",
            Self::ProviderRequestBody => "provider_request_body_ref",
            Self::ResponseBody => "response_body_ref",
            Self::ClientResponseBody => "client_response_body_ref",
        }
    }

    pub fn as_storage_field(&self) -> &'static str {
        match self {
            Self::RequestBody => "request_body",
            Self::ProviderRequestBody => "provider_request_body",
            Self::ResponseBody => "response_body",
            Self::ClientResponseBody => "client_response_body",
        }
    }

    pub fn from_storage_field(value: &str) -> Option<Self> {
        match value {
            "request_body" => Some(Self::RequestBody),
            "provider_request_body" => Some(Self::ProviderRequestBody),
            "response_body" => Some(Self::ResponseBody),
            "client_response_body" => Some(Self::ClientResponseBody),
            _ => None,
        }
    }
}

pub fn usage_body_ref(request_id: &str, field: UsageBodyField) -> String {
    format!("usage://request/{request_id}/{}", field.as_storage_field())
}

pub fn parse_usage_body_ref(body_ref: &str) -> Option<(String, UsageBodyField)> {
    let body_ref = body_ref.trim();
    let prefix = "usage://request/";
    let suffix = body_ref.strip_prefix(prefix)?;
    let (request_id, field) = suffix.rsplit_once('/')?;
    let request_id = request_id.trim();
    if request_id.is_empty() {
        return None;
    }
    Some((
        request_id.to_string(),
        UsageBodyField::from_storage_field(field.trim())?,
    ))
}

#[async_trait]
pub trait UsageReadRepository: Send + Sync {
    async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, crate::DataLayerError>;

    async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, crate::DataLayerError>;

    async fn resolve_body_ref(
        &self,
        body_ref: &str,
    ) -> Result<Option<Value>, crate::DataLayerError>;

    async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, crate::DataLayerError>;

    async fn count_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<u64, crate::DataLayerError>;

    async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, crate::DataLayerError>;

    async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, crate::DataLayerError>;

    async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, crate::DataLayerError>;

    async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, crate::DataLayerError>;

    async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, crate::DataLayerError>;

    async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, crate::DataLayerError>;

    async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<
        std::collections::BTreeMap<String, StoredProviderApiKeyUsageSummary>,
        crate::DataLayerError,
    >;

    async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, crate::DataLayerError>;

    async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, crate::DataLayerError>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertUsageRecord {
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub provider_name: String,
    pub model: String,
    pub target_model: Option<String>,
    pub provider_id: Option<String>,
    pub provider_endpoint_id: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub request_type: Option<String>,
    pub api_format: Option<String>,
    pub api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub endpoint_api_format: Option<String>,
    pub provider_api_family: Option<String>,
    pub provider_endpoint_kind: Option<String>,
    pub has_format_conversion: Option<bool>,
    pub is_stream: Option<bool>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_creation_ephemeral_5m_input_tokens: Option<u64>,
    pub cache_creation_ephemeral_1h_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_cost_usd: Option<f64>,
    pub cache_read_cost_usd: Option<f64>,
    pub output_price_per_1m: Option<f64>,
    pub total_cost_usd: Option<f64>,
    pub actual_total_cost_usd: Option<f64>,
    pub status_code: Option<u16>,
    pub error_message: Option<String>,
    pub error_category: Option<String>,
    pub response_time_ms: Option<u64>,
    pub first_byte_time_ms: Option<u64>,
    pub status: String,
    pub billing_status: String,
    pub request_headers: Option<Value>,
    pub request_body: Option<Value>,
    pub request_body_ref: Option<String>,
    pub provider_request_headers: Option<Value>,
    pub provider_request_body: Option<Value>,
    pub provider_request_body_ref: Option<String>,
    pub response_headers: Option<Value>,
    pub response_body: Option<Value>,
    pub response_body_ref: Option<String>,
    pub client_response_headers: Option<Value>,
    pub client_response_body: Option<Value>,
    pub client_response_body_ref: Option<String>,
    pub candidate_id: Option<String>,
    pub candidate_index: Option<u64>,
    pub key_name: Option<String>,
    pub planner_kind: Option<String>,
    pub route_family: Option<String>,
    pub route_kind: Option<String>,
    pub execution_path: Option<String>,
    pub local_execution_runtime_miss_reason: Option<String>,
    pub request_metadata: Option<Value>,
    pub finalized_at_unix_secs: Option<u64>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: u64,
}

impl UpsertUsageRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.request_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "usage upsert request_id cannot be empty".to_string(),
            ));
        }
        if self.provider_name.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "usage upsert provider_name cannot be empty".to_string(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "usage upsert model cannot be empty".to_string(),
            ));
        }
        if self.status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "usage upsert status cannot be empty".to_string(),
            ));
        }
        if self.billing_status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "usage upsert billing_status cannot be empty".to_string(),
            ));
        }
        if let Some(value) = self.total_cost_usd {
            if !value.is_finite() {
                return Err(crate::DataLayerError::InvalidInput(
                    "usage upsert total_cost_usd must be finite".to_string(),
                ));
            }
        }
        if let Some(value) = self.cache_creation_cost_usd {
            if !value.is_finite() {
                return Err(crate::DataLayerError::InvalidInput(
                    "usage upsert cache_creation_cost_usd must be finite".to_string(),
                ));
            }
        }
        if let Some(value) = self.cache_read_cost_usd {
            if !value.is_finite() {
                return Err(crate::DataLayerError::InvalidInput(
                    "usage upsert cache_read_cost_usd must be finite".to_string(),
                ));
            }
        }
        if let Some(value) = self.output_price_per_1m {
            if !value.is_finite() {
                return Err(crate::DataLayerError::InvalidInput(
                    "usage upsert output_price_per_1m must be finite".to_string(),
                ));
            }
        }
        if let Some(value) = self.actual_total_cost_usd {
            if !value.is_finite() {
                return Err(crate::DataLayerError::InvalidInput(
                    "usage upsert actual_total_cost_usd must be finite".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[async_trait]
pub trait UsageWriteRepository: Send + Sync {
    async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, crate::DataLayerError>;
}

pub trait UsageRepository: UsageReadRepository + UsageWriteRepository + Send + Sync {}

impl<T> UsageRepository for T where T: UsageReadRepository + UsageWriteRepository + Send + Sync {}

fn parse_u64(value: i32, field_name: &str) -> Result<u64, crate::DataLayerError> {
    u64::try_from(value).map_err(|_| {
        crate::DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
    })
}

fn parse_optional_u64(
    value: Option<i32>,
    field_name: &str,
) -> Result<Option<u64>, crate::DataLayerError> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                crate::DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
            })
        })
        .transpose()
}

fn parse_u16(value: Option<i32>, field_name: &str) -> Result<Option<u16>, crate::DataLayerError> {
    value
        .map(|value| {
            u16::try_from(value).map_err(|_| {
                crate::DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
            })
        })
        .transpose()
}

fn parse_timestamp(value: i64, field_name: &str) -> Result<u64, crate::DataLayerError> {
    u64::try_from(value).map_err(|_| {
        crate::DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
    })
}

#[cfg(test)]
mod tests {
    use super::{StoredRequestUsageAudit, UpsertUsageRecord, UsageBodyField};
    use serde_json::json;

    fn sample_usage() -> StoredRequestUsageAudit {
        StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            "OpenAI".to_string(),
            "gpt-4.1".to_string(),
            None,
            None,
            None,
            None,
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            10,
            20,
            30,
            0.1,
            0.1,
            Some(200),
            None,
            None,
            Some(120),
            Some(80),
            "completed".to_string(),
            "settled".to_string(),
            100,
            101,
            Some(102),
        )
        .expect("usage should build")
    }

    #[test]
    fn rejects_empty_request_id() {
        assert!(StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "".to_string(),
            None,
            None,
            None,
            None,
            "OpenAI".to_string(),
            "gpt-4.1".to_string(),
            None,
            None,
            None,
            None,
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            10,
            20,
            30,
            0.1,
            0.1,
            Some(200),
            None,
            None,
            Some(120),
            Some(80),
            "completed".to_string(),
            "settled".to_string(),
            100,
            101,
            Some(102),
        )
        .is_err());
    }

    #[test]
    fn rejects_negative_token_count() {
        assert!(StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            "OpenAI".to_string(),
            "gpt-4.1".to_string(),
            None,
            None,
            None,
            None,
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            -1,
            20,
            30,
            0.1,
            0.1,
            Some(200),
            None,
            None,
            Some(120),
            Some(80),
            "completed".to_string(),
            "settled".to_string(),
            100,
            101,
            Some(102),
        )
        .is_err());
    }

    #[test]
    fn rejects_invalid_upsert_payload() {
        let record = UpsertUsageRecord {
            request_id: "".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "openai".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: None,
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: Some(30),
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: None,
            actual_total_cost_usd: None,
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(120),
            first_byte_time_ms: None,
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: Some(json!({"authorization": "Bearer test"})),
            request_body: Some(json!({"model": "gpt-5"})),
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        };

        assert!(record.validate().is_err());
    }

    #[test]
    fn settlement_accessors_prefer_typed_metadata() {
        let mut usage = sample_usage();
        usage.output_price_per_1m = Some(11.0);
        usage.request_metadata = Some(json!({
            "billing_snapshot_schema_version": "v2",
            "billing_snapshot_status": "resolved",
            "rate_multiplier": 0.5,
            "is_free_tier": false,
            "input_price_per_1m": 3.0,
            "output_price_per_1m": 9.0,
            "cache_creation_price_per_1m": 3.75,
            "cache_read_price_per_1m": 0.3,
            "price_per_request": 0.02,
            "billing_snapshot": {
                "resolved_variables": {
                    "output_price_per_1m": 11.0
                }
            }
        }));

        assert_eq!(
            usage.settlement_billing_snapshot_schema_version(),
            Some("v2")
        );
        assert_eq!(usage.settlement_billing_snapshot_status(), Some("resolved"));
        assert_eq!(usage.settlement_rate_multiplier(), Some(0.5));
        assert_eq!(usage.settlement_is_free_tier(), Some(false));
        assert_eq!(usage.settlement_input_price_per_1m(), Some(3.0));
        assert_eq!(usage.settlement_output_price_per_1m(), Some(9.0));
        assert_eq!(usage.settlement_cache_creation_price_per_1m(), Some(3.75));
        assert_eq!(usage.settlement_cache_read_price_per_1m(), Some(0.3));
        assert_eq!(usage.settlement_price_per_request(), Some(0.02));
    }

    #[test]
    fn settlement_accessors_fall_back_to_billing_snapshot_and_legacy_output_price() {
        let mut usage = sample_usage();
        usage.output_price_per_1m = Some(15.0);
        usage.request_metadata = Some(json!({
            "billing_snapshot": {
                "resolved_variables": {
                    "input_price_per_1m": 3.0,
                    "cache_creation_price_per_1m": 3.75,
                    "cache_read_price_per_1m": 0.3,
                    "price_per_request": 0.02
                }
            }
        }));

        assert_eq!(usage.settlement_input_price_per_1m(), Some(3.0));
        assert_eq!(usage.settlement_output_price_per_1m(), Some(15.0));
        assert_eq!(usage.settlement_cache_creation_price_per_1m(), Some(3.75));
        assert_eq!(usage.settlement_cache_read_price_per_1m(), Some(0.3));
        assert_eq!(usage.settlement_price_per_request(), Some(0.02));
    }

    #[test]
    fn body_ref_and_routing_accessors_prefer_typed_fields() {
        let mut usage = sample_usage();
        usage.request_body_ref = Some("usage://request/req-1/request_body".to_string());
        usage.provider_request_body_ref =
            Some("usage://request/req-1/provider_request_body".to_string());
        usage.response_body_ref = Some("usage://request/req-1/response_body".to_string());
        usage.client_response_body_ref =
            Some("usage://request/req-1/client_response_body".to_string());
        usage.candidate_id = Some("cand-typed".to_string());
        usage.key_name = Some("primary-typed".to_string());
        usage.planner_kind = Some("claude_cli_sync".to_string());
        usage.route_family = Some("claude".to_string());
        usage.route_kind = Some("cli".to_string());
        usage.execution_path = Some("local_execution_runtime_miss".to_string());
        usage.local_execution_runtime_miss_reason = Some("all_candidates_skipped".to_string());
        usage.request_metadata = Some(json!({
            "request_body_ref": "blob://legacy-request",
            "provider_request_body_ref": "blob://legacy-provider",
            "response_body_ref": "blob://legacy-response",
            "client_response_body_ref": "blob://legacy-client-response",
            "candidate_id": "cand-legacy",
            "key_name": "primary-legacy"
        }));

        assert_eq!(
            usage.body_ref(UsageBodyField::RequestBody),
            Some("usage://request/req-1/request_body")
        );
        assert_eq!(
            usage.body_ref(UsageBodyField::ProviderRequestBody),
            Some("usage://request/req-1/provider_request_body")
        );
        assert_eq!(
            usage.body_ref(UsageBodyField::ResponseBody),
            Some("usage://request/req-1/response_body")
        );
        assert_eq!(
            usage.body_ref(UsageBodyField::ClientResponseBody),
            Some("usage://request/req-1/client_response_body")
        );
        assert_eq!(usage.routing_candidate_id(), Some("cand-typed"));
        assert_eq!(usage.routing_key_name(), Some("primary-typed"));
        assert_eq!(usage.routing_planner_kind(), Some("claude_cli_sync"));
        assert_eq!(usage.routing_route_family(), Some("claude"));
        assert_eq!(usage.routing_route_kind(), Some("cli"));
        assert_eq!(
            usage.routing_execution_path(),
            Some("local_execution_runtime_miss")
        );
        assert_eq!(
            usage.routing_local_execution_runtime_miss_reason(),
            Some("all_candidates_skipped")
        );
    }

    #[test]
    fn body_ref_accessor_ignores_legacy_metadata_compatibility_keys() {
        let mut usage = sample_usage();
        usage.request_metadata = Some(json!({
            "request_body_ref": "blob://legacy-request",
            "provider_request_body_ref": "blob://legacy-provider",
            "response_body_ref": "blob://legacy-response",
            "client_response_body_ref": "blob://legacy-client-response"
        }));

        assert_eq!(usage.body_ref(UsageBodyField::RequestBody), None);
        assert_eq!(usage.body_ref(UsageBodyField::ProviderRequestBody), None);
        assert_eq!(usage.body_ref(UsageBodyField::ResponseBody), None);
        assert_eq!(usage.body_ref(UsageBodyField::ClientResponseBody), None);
    }
}
