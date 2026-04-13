use crate::handlers::admin::shared::{
    deserialize_optional_f64_from_number_or_string, AdminTypedObjectPatch,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct AdminGlobalModelCreateRequest {
    pub(crate) name: String,
    pub(crate) display_name: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) default_price_per_request: Option<f64>,
    #[serde(default)]
    pub(crate) default_tiered_pricing: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) supported_capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminGlobalModelUpdateRequest {
    #[serde(default)]
    pub(crate) display_name: Option<String>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) default_price_per_request: Option<f64>,
    #[serde(default)]
    pub(crate) default_tiered_pricing: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) supported_capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
}

pub(crate) type AdminGlobalModelUpdatePatch = AdminTypedObjectPatch<AdminGlobalModelUpdateRequest>;

#[derive(Debug, Deserialize)]
pub(crate) struct AdminBatchDeleteIdsRequest {
    pub(crate) ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminBatchAssignToProvidersRequest {
    pub(crate) provider_ids: Vec<String>,
    #[serde(default)]
    pub(crate) create_models: Option<bool>,
}
