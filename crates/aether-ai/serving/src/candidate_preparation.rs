#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiPreparedHeaderAuthenticatedCandidate {
    pub auth_header: String,
    pub auth_value: String,
    pub mapped_model: String,
}

pub fn prepare_ai_header_authenticated_candidate(
    direct_auth: Option<(String, String)>,
    oauth_header_auth: Option<(String, String)>,
    selected_provider_model_name: &str,
) -> Result<AiPreparedHeaderAuthenticatedCandidate, &'static str> {
    let Some((auth_header, auth_value)) = direct_auth.or(oauth_header_auth) else {
        return Err("transport_auth_unavailable");
    };
    let mapped_model = resolve_ai_candidate_mapped_model(selected_provider_model_name)?;

    Ok(AiPreparedHeaderAuthenticatedCandidate {
        auth_header,
        auth_value,
        mapped_model,
    })
}

pub fn resolve_ai_candidate_mapped_model(
    selected_provider_model_name: &str,
) -> Result<String, &'static str> {
    let mapped_model = selected_provider_model_name.trim().to_string();
    if mapped_model.is_empty() {
        return Err("mapped_model_missing");
    }

    Ok(mapped_model)
}

#[cfg(test)]
mod tests {
    use super::{prepare_ai_header_authenticated_candidate, resolve_ai_candidate_mapped_model};

    #[test]
    fn mapped_model_trims_selected_provider_model_name() {
        assert_eq!(
            resolve_ai_candidate_mapped_model("  gpt-test-upstream  "),
            Ok("gpt-test-upstream".to_string())
        );
    }

    #[test]
    fn mapped_model_rejects_empty_selected_provider_model_name() {
        assert_eq!(
            resolve_ai_candidate_mapped_model("  "),
            Err("mapped_model_missing")
        );
    }

    #[test]
    fn header_auth_preparation_prefers_direct_auth_and_allows_empty_value() {
        let prepared = prepare_ai_header_authenticated_candidate(
            Some(("authorization".to_string(), String::new())),
            Some(("x-oauth".to_string(), "oauth".to_string())),
            "gpt-test-upstream",
        )
        .expect("direct auth should prepare candidate");

        assert_eq!(prepared.auth_header, "authorization");
        assert_eq!(prepared.auth_value, "");
        assert_eq!(prepared.mapped_model, "gpt-test-upstream");
    }

    #[test]
    fn header_auth_preparation_falls_back_to_oauth_header_auth() {
        let prepared = prepare_ai_header_authenticated_candidate(
            None,
            Some(("authorization".to_string(), "Bearer token".to_string())),
            " gpt-test-upstream ",
        )
        .expect("oauth header auth should prepare candidate");

        assert_eq!(prepared.auth_header, "authorization");
        assert_eq!(prepared.auth_value, "Bearer token");
        assert_eq!(prepared.mapped_model, "gpt-test-upstream");
    }

    #[test]
    fn header_auth_preparation_requires_some_auth() {
        assert_eq!(
            prepare_ai_header_authenticated_candidate(None, None, "gpt-test-upstream"),
            Err("transport_auth_unavailable")
        );
    }
}
