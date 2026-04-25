use aether_contracts::ExecutionPlan;
use tracing::warn;

use crate::AppState;

pub(crate) async fn refresh_oauth_plan_auth_for_retry(
    state: &AppState,
    plan: &mut ExecutionPlan,
    status_code: u16,
    response_text: Option<&str>,
    trace_id: &str,
) -> bool {
    if !status_may_be_oauth_invalid(status_code, response_text) {
        return false;
    }

    let transport = match state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return false,
        Err(err) => {
            warn!(
                event_name = "local_oauth_retry_transport_read_failed",
                log_type = "ops",
                trace_id = %trace_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                error = ?err,
                "gateway failed to read transport before oauth retry refresh"
            );
            return false;
        }
    };

    if transport.key.decrypted_auth_config.is_none()
        && !transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
    {
        return false;
    }

    match state.force_local_oauth_refresh_entry(&transport).await {
        Ok(Some(entry)) => {
            let header_name = entry.auth_header_name.trim().to_ascii_lowercase();
            let header_value = entry.auth_header_value.trim();
            if header_name.is_empty() || header_value.is_empty() {
                return false;
            }
            plan.headers.insert(header_name, header_value.to_string());
            true
        }
        Ok(None) => false,
        Err(err) => {
            warn!(
                event_name = "local_oauth_retry_refresh_failed",
                log_type = "ops",
                trace_id = %trace_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                status_code,
                error = %err,
                "gateway oauth retry refresh failed"
            );
            false
        }
    }
}

fn status_may_be_oauth_invalid(status_code: u16, response_text: Option<&str>) -> bool {
    if status_code == 401 {
        return true;
    }
    if status_code != 403 {
        return false;
    }

    let Some(response_text) = response_text else {
        return true;
    };
    let response_text = response_text.to_ascii_lowercase();
    ["oauth", "token", "auth", "credential", "expired"]
        .iter()
        .any(|needle| response_text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::status_may_be_oauth_invalid;

    #[test]
    fn recognizes_oauth_invalid_statuses() {
        assert!(status_may_be_oauth_invalid(401, None));
        assert!(status_may_be_oauth_invalid(
            403,
            Some("The security token included in the request is expired")
        ));
        assert!(status_may_be_oauth_invalid(403, None));
        assert!(!status_may_be_oauth_invalid(403, Some("quota exceeded")));
        assert!(!status_may_be_oauth_invalid(429, Some("token bucket")));
    }
}
