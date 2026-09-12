use aether_routing_core::{
    apply_json_patch_operations, validate_header_patch, MutationError, MutationPlan,
    RoutingHeaderPatch,
};
use http::StatusCode;
use http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::GatewayError;

const INVALID_ROUTING_MUTATION_MESSAGE: &str = "invalid routing mutation";

pub(crate) fn apply_routing_mutation_plan(
    body: &mut Value,
    headers: &mut HeaderMap,
    plan: &MutationPlan,
) -> Result<(), GatewayError> {
    apply_json_patch_operations(body, &plan.body_patch).map_err(|_| invalid_routing_mutation())?;
    apply_header_patch(headers, &plan.header_patch).map_err(|_| invalid_routing_mutation())?;
    Ok(())
}

fn invalid_routing_mutation() -> GatewayError {
    GatewayError::Client {
        status: StatusCode::BAD_REQUEST,
        message: INVALID_ROUTING_MUTATION_MESSAGE.to_string(),
    }
}

fn apply_header_patch(
    headers: &mut HeaderMap,
    patch: &[RoutingHeaderPatch],
) -> Result<(), MutationError> {
    validate_header_patch(patch)?;
    for item in patch {
        match item {
            RoutingHeaderPatch::Set { name, value } => {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| MutationError::InvalidHeaderName(name.clone()))?;
                let value = HeaderValue::from_str(value)
                    .map_err(|_| MutationError::InvalidHeaderName(name.to_string()))?;
                headers.insert(name, value);
            }
            RoutingHeaderPatch::Remove { name } => {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| MutationError::InvalidHeaderName(name.clone()))?;
                headers.remove(name);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aether_routing_core::RoutingJsonPatchOperation;
    use serde_json::json;

    use super::*;

    #[test]
    fn body_mutation_errors_do_not_echo_json_pointer() {
        let secret = "https://internal.example/?token=Bearer-secret";
        let plan = MutationPlan {
            body_patch: vec![RoutingJsonPatchOperation::Replace {
                path: secret.to_string(),
                value: json!("replacement"),
            }],
            ..MutationPlan::default()
        };

        let error = apply_routing_mutation_plan(
            &mut json!({"model": "test"}),
            &mut HeaderMap::new(),
            &plan,
        )
        .expect_err("invalid pointer should fail");

        assert!(matches!(
            error,
            GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ref message,
            } if message == INVALID_ROUTING_MUTATION_MESSAGE && !message.contains(secret)
        ));
    }

    #[test]
    fn header_mutation_errors_do_not_echo_header_name() {
        let secret = "Authorization: Bearer secret";
        let plan = MutationPlan {
            header_patch: vec![RoutingHeaderPatch::Remove {
                name: secret.to_string(),
            }],
            ..MutationPlan::default()
        };

        let error = apply_routing_mutation_plan(
            &mut json!({"model": "test"}),
            &mut HeaderMap::new(),
            &plan,
        )
        .expect_err("invalid header should fail");

        assert!(matches!(
            error,
            GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ref message,
            } if message == INVALID_ROUTING_MUTATION_MESSAGE && !message.contains(secret)
        ));
    }
}
