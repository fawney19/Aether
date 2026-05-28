use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum WebhookSecretUpdate {
    #[default]
    Preserve,
    Clear,
    Set(String),
}

impl WebhookSecretUpdate {
    pub fn mode_name(&self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Clear => "clear",
            Self::Set(_) => "set",
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Set(value) => Some(value.as_str()),
            Self::Preserve | Self::Clear => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredWebhookEndpoint {
    pub id: String,
    pub name: String,
    pub url: String,
    pub secret_encrypted: Option<String>,
    pub enabled: bool,
    pub event_filter_json: Option<serde_json::Value>,
    pub timeout_ms: u32,
    pub max_attempts: u32,
    pub initial_retry_delay_seconds: u32,
    pub max_retry_delay_seconds: u32,
    pub last_test_delivery_id: Option<String>,
    pub last_test_status: Option<String>,
    pub last_test_error: Option<String>,
    pub last_test_at_unix_secs: Option<u64>,
    pub created_by: Option<String>,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

impl StoredWebhookEndpoint {
    pub fn subscribes_to(&self, event_type: &str) -> bool {
        event_filter_matches(self.event_filter_json.as_ref(), event_type)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertWebhookEndpointRecord {
    pub id: Option<String>,
    pub name: String,
    pub url: String,
    pub secret_encrypted: WebhookSecretUpdate,
    pub enabled: bool,
    pub event_filter_json: Option<serde_json::Value>,
    pub timeout_ms: u32,
    pub max_attempts: u32,
    pub initial_retry_delay_seconds: u32,
    pub max_retry_delay_seconds: u32,
    pub created_by: Option<String>,
}

impl UpsertWebhookEndpointRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self
            .id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook endpoint id cannot be empty".to_string(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook endpoint name is required".to_string(),
            ));
        }
        if self.url.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook endpoint url is required".to_string(),
            ));
        }
        validate_positive_u32(self.timeout_ms, "webhook endpoint timeout_ms")?;
        validate_positive_u32(self.max_attempts, "webhook endpoint max_attempts")?;
        validate_positive_u32(
            self.initial_retry_delay_seconds,
            "webhook endpoint initial_retry_delay_seconds",
        )?;
        validate_positive_u32(
            self.max_retry_delay_seconds,
            "webhook endpoint max_retry_delay_seconds",
        )?;
        validate_event_filter(self.event_filter_json.as_ref())?;
        Ok(())
    }

    pub fn effective_id(&self) -> String {
        self.id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WebhookEndpointListQuery {
    pub enabled: Option<bool>,
    pub event_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredWebhookDelivery {
    pub id: String,
    pub endpoint_id: String,
    pub event_id: String,
    pub event_type: String,
    pub is_test: bool,
    pub payload_json: serde_json::Value,
    pub status: String,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub next_attempt_at_unix_secs: u64,
    pub last_attempt_at_unix_secs: Option<u64>,
    pub delivered_at_unix_secs: Option<u64>,
    pub failed_at_unix_secs: Option<u64>,
    pub last_http_status: Option<i32>,
    pub last_error: Option<String>,
    pub response_excerpt: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at_unix_secs: Option<u64>,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateWebhookDeliveryRecord {
    pub id: Option<String>,
    pub endpoint_id: String,
    pub event_id: String,
    pub event_type: String,
    pub is_test: bool,
    pub payload_json: serde_json::Value,
    pub max_attempts: u32,
    pub next_attempt_at_unix_secs: u64,
}

impl CreateWebhookDeliveryRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self
            .id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery id cannot be empty".to_string(),
            ));
        }
        if self.endpoint_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery endpoint_id is required".to_string(),
            ));
        }
        if self.event_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery event_id is required".to_string(),
            ));
        }
        if self.event_type.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery event_type is required".to_string(),
            ));
        }
        validate_positive_u32(self.max_attempts, "webhook delivery max_attempts")?;
        Ok(())
    }

    pub fn effective_id(&self) -> String {
        self.id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WebhookDeliveryListQuery {
    pub endpoint_id: Option<String>,
    pub status: Option<String>,
    pub event_type: Option<String>,
    pub is_test: Option<bool>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredWebhookDeliveryPage {
    pub items: Vec<StoredWebhookDelivery>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaimWebhookDeliveriesRequest {
    pub owner: String,
    pub now_unix_secs: u64,
    pub lease_seconds: u64,
    pub limit: usize,
}

impl ClaimWebhookDeliveriesRequest {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.owner.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery lease owner is required".to_string(),
            ));
        }
        if self.lease_seconds == 0 {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery lease_seconds must be positive".to_string(),
            ));
        }
        if self.limit == 0 {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery claim limit must be positive".to_string(),
            ));
        }
        Ok(())
    }

    pub fn lease_expires_at_unix_secs(&self) -> u64 {
        self.now_unix_secs.saturating_add(self.lease_seconds)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredWebhookDeliveryAttempt {
    pub id: String,
    pub delivery_id: String,
    pub attempt_index: u32,
    pub status: String,
    pub http_status: Option<i32>,
    pub error_message: Option<String>,
    pub response_excerpt: Option<String>,
    pub started_at_unix_secs: u64,
    pub completed_at_unix_secs: Option<u64>,
    pub duration_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordWebhookDeliveryAttemptRecord {
    pub id: Option<String>,
    pub delivery_id: String,
    pub attempt_index: u32,
    pub status: String,
    pub http_status: Option<i32>,
    pub error_message: Option<String>,
    pub response_excerpt: Option<String>,
    pub started_at_unix_secs: u64,
    pub completed_at_unix_secs: Option<u64>,
    pub duration_ms: Option<u32>,
    pub delivery_status: String,
    pub next_attempt_at_unix_secs: Option<u64>,
    pub delivered_at_unix_secs: Option<u64>,
    pub failed_at_unix_secs: Option<u64>,
}

impl RecordWebhookDeliveryAttemptRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self
            .id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery attempt id cannot be empty".to_string(),
            ));
        }
        if self.delivery_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery attempt delivery_id is required".to_string(),
            ));
        }
        if self.status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery attempt status is required".to_string(),
            ));
        }
        if self.delivery_status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery status is required".to_string(),
            ));
        }
        Ok(())
    }

    pub fn effective_id(&self) -> String {
        self.id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    }

    pub fn completed_or_started_at(&self) -> u64 {
        self.completed_at_unix_secs
            .unwrap_or(self.started_at_unix_secs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RescheduleWebhookDeliveryRecord {
    pub delivery_id: String,
    pub status: String,
    pub next_attempt_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

impl RescheduleWebhookDeliveryRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.delivery_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery id is required".to_string(),
            ));
        }
        if self.status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook delivery status is required".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WebhookDeliveryAttemptListQuery {
    pub delivery_id: String,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateWebhookEndpointTestResultRecord {
    pub endpoint_id: String,
    pub delivery_id: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub tested_at_unix_secs: u64,
}

impl UpdateWebhookEndpointTestResultRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.endpoint_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook endpoint test endpoint_id is required".to_string(),
            ));
        }
        if self.status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "webhook endpoint test status is required".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
pub trait WebhookNotificationReadRepository: Send + Sync {
    async fn list_webhook_endpoints(
        &self,
        query: &WebhookEndpointListQuery,
    ) -> Result<Vec<StoredWebhookEndpoint>, crate::DataLayerError>;

    async fn find_webhook_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<Option<StoredWebhookEndpoint>, crate::DataLayerError>;

    async fn find_webhook_delivery(
        &self,
        delivery_id: &str,
    ) -> Result<Option<StoredWebhookDelivery>, crate::DataLayerError>;

    async fn list_webhook_deliveries(
        &self,
        query: &WebhookDeliveryListQuery,
    ) -> Result<StoredWebhookDeliveryPage, crate::DataLayerError>;

    async fn list_webhook_delivery_attempts(
        &self,
        query: &WebhookDeliveryAttemptListQuery,
    ) -> Result<Vec<StoredWebhookDeliveryAttempt>, crate::DataLayerError>;
}

#[async_trait]
pub trait WebhookNotificationWriteRepository: Send + Sync {
    async fn upsert_webhook_endpoint(
        &self,
        record: &UpsertWebhookEndpointRecord,
    ) -> Result<StoredWebhookEndpoint, crate::DataLayerError>;

    async fn delete_webhook_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn create_webhook_delivery(
        &self,
        record: &CreateWebhookDeliveryRecord,
    ) -> Result<StoredWebhookDelivery, crate::DataLayerError>;

    async fn claim_due_webhook_deliveries(
        &self,
        request: &ClaimWebhookDeliveriesRequest,
    ) -> Result<Vec<StoredWebhookDelivery>, crate::DataLayerError>;

    async fn record_webhook_delivery_attempt(
        &self,
        record: &RecordWebhookDeliveryAttemptRecord,
    ) -> Result<StoredWebhookDeliveryAttempt, crate::DataLayerError>;

    async fn reschedule_webhook_delivery(
        &self,
        record: &RescheduleWebhookDeliveryRecord,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_webhook_endpoint_test_result(
        &self,
        record: &UpdateWebhookEndpointTestResultRecord,
    ) -> Result<bool, crate::DataLayerError>;
}

pub trait WebhookNotificationRepository:
    WebhookNotificationReadRepository + WebhookNotificationWriteRepository + Send + Sync
{
}

impl<T> WebhookNotificationRepository for T where
    T: WebhookNotificationReadRepository + WebhookNotificationWriteRepository + Send + Sync
{
}

pub(crate) fn event_filter_matches(filter: Option<&serde_json::Value>, event_type: &str) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    match filter {
        serde_json::Value::Null => true,
        serde_json::Value::Array(events) => events.iter().any(|event| {
            event
                .as_str()
                .is_some_and(|candidate| candidate == "*" || candidate == event_type)
        }),
        serde_json::Value::String(value) => value == "*" || value == event_type,
        _ => false,
    }
}

fn validate_event_filter(filter: Option<&serde_json::Value>) -> Result<(), crate::DataLayerError> {
    let Some(filter) = filter else {
        return Ok(());
    };
    match filter {
        serde_json::Value::Null => Ok(()),
        serde_json::Value::String(value) if !value.trim().is_empty() => Ok(()),
        serde_json::Value::Array(events) => {
            for event in events {
                let Some(value) = event.as_str() else {
                    return Err(crate::DataLayerError::InvalidInput(
                        "webhook event_filter_json must contain only event names".to_string(),
                    ));
                };
                if value.trim().is_empty() {
                    return Err(crate::DataLayerError::InvalidInput(
                        "webhook event_filter_json cannot contain empty event names".to_string(),
                    ));
                }
            }
            Ok(())
        }
        _ => Err(crate::DataLayerError::InvalidInput(
            "webhook event_filter_json must be null, a string, or a string array".to_string(),
        )),
    }
}

fn validate_positive_u32(value: u32, field_name: &str) -> Result<(), crate::DataLayerError> {
    if value == 0 {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field_name} must be positive"
        )));
    }
    Ok(())
}
