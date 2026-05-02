use serde_json::Value;

use crate::GatewayError;

pub(crate) use crate::ai_serving::pure::SyncToStreamBridgeOutcome;

pub(crate) fn maybe_bridge_standard_sync_json_to_stream(
    provider_body_json: &Value,
    provider_api_format: &str,
    client_api_format: &str,
    report_context: Option<&Value>,
) -> Result<Option<SyncToStreamBridgeOutcome>, GatewayError> {
    crate::ai_serving::pure::maybe_bridge_standard_sync_json_to_stream(
        provider_body_json,
        provider_api_format,
        client_api_format,
        report_context,
    )
    .map_err(GatewayError::from)
}
