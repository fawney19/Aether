use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SchedulerPriorityMode {
    #[default]
    Provider,
    GlobalKey,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SchedulerMinimalCandidateSelectionCandidate {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: String,
    pub provider_priority: i32,
    pub endpoint_id: String,
    pub endpoint_api_format: String,
    pub key_id: String,
    pub key_name: String,
    pub key_auth_type: String,
    pub key_internal_priority: i32,
    pub key_global_priority_for_format: Option<i32>,
    pub key_capabilities: Option<serde_json::Value>,
    pub model_id: String,
    pub global_model_id: String,
    pub global_model_name: String,
    pub selected_provider_model_name: String,
    pub mapping_matched_model: Option<String>,
}

pub struct EnumerateMinimalCandidateSelectionInput<'a> {
    pub rows: Vec<StoredMinimalCandidateSelectionRow>,
    pub normalized_api_format: &'a str,
    pub requested_model_name: &'a str,
    pub resolved_global_model_name: &'a str,
    pub require_streaming: bool,
    pub required_capabilities: Option<&'a serde_json::Value>,
    pub auth_constraints: Option<&'a crate::SchedulerAuthConstraints>,
}
