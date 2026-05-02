#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulerAuthConstraints {
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_models: Option<Vec<String>>,
}

pub fn provider_matches_allowed_value(
    allowed_value: &str,
    provider_id: &str,
    provider_name: &str,
    provider_type: &str,
) -> bool {
    let allowed_value = allowed_value.trim();
    !allowed_value.is_empty()
        && (allowed_value.eq_ignore_ascii_case(provider_id.trim())
            || allowed_value.eq_ignore_ascii_case(provider_name.trim())
            || allowed_value.eq_ignore_ascii_case(provider_type.trim()))
}

pub fn auth_constraints_allow_provider(
    constraints: Option<&SchedulerAuthConstraints>,
    provider_id: &str,
    provider_name: &str,
    provider_type: &str,
) -> bool {
    let Some(allowed) =
        constraints.and_then(|constraints| constraints.allowed_providers.as_deref())
    else {
        return true;
    };

    allowed.iter().any(|value| {
        provider_matches_allowed_value(value, provider_id, provider_name, provider_type)
    })
}

pub fn auth_constraints_allow_api_format(
    constraints: Option<&SchedulerAuthConstraints>,
    api_format: &str,
) -> bool {
    let Some(allowed) =
        constraints.and_then(|constraints| constraints.allowed_api_formats.as_deref())
    else {
        return true;
    };

    allowed
        .iter()
        .any(|value| api_format_matches_allowed_value(value, api_format))
}

pub fn api_format_matches_allowed_value(allowed_value: &str, api_format: &str) -> bool {
    let allowed_value = allowed_value.trim();
    let api_format = api_format.trim();
    if allowed_value.is_empty() || api_format.is_empty() {
        return false;
    }
    crate::normalize_api_format(allowed_value) == crate::normalize_api_format(api_format)
}

pub fn auth_constraints_allow_model(
    constraints: Option<&SchedulerAuthConstraints>,
    requested_model_name: &str,
    resolved_global_model_name: &str,
) -> bool {
    let Some(allowed) = constraints.and_then(|constraints| constraints.allowed_models.as_deref())
    else {
        return true;
    };

    allowed
        .iter()
        .any(|value| value == requested_model_name || value == resolved_global_model_name)
}

#[cfg(test)]
mod tests {
    use super::{
        api_format_matches_allowed_value, auth_constraints_allow_api_format,
        auth_constraints_allow_model, auth_constraints_allow_provider,
        provider_matches_allowed_value, SchedulerAuthConstraints,
    };

    fn sample_constraints() -> SchedulerAuthConstraints {
        SchedulerAuthConstraints {
            allowed_providers: Some(vec!["provider-1".to_string(), "OpenAI".to_string()]),
            allowed_api_formats: Some(vec!["OPENAI:CHAT".to_string()]),
            allowed_models: Some(vec!["gpt-5".to_string()]),
        }
    }

    #[test]
    fn constraints_allow_matching_provider_identifier_or_name() {
        let constraints = sample_constraints();
        assert!(auth_constraints_allow_provider(
            Some(&constraints),
            "provider-1",
            "other",
            "other",
        ));
        assert!(auth_constraints_allow_provider(
            Some(&constraints),
            "other",
            "openai",
            "other",
        ));
        assert!(auth_constraints_allow_provider(
            Some(&constraints),
            "other",
            "other",
            "openai",
        ));
        assert!(!auth_constraints_allow_provider(
            Some(&constraints),
            "other",
            "other",
            "other",
        ));
    }

    #[test]
    fn provider_allowed_value_matches_type() {
        assert!(provider_matches_allowed_value(
            "openai",
            "provider-1",
            "OpenAI Pool",
            "openai",
        ));
        assert!(!provider_matches_allowed_value(
            "claude",
            "provider-1",
            "OpenAI Pool",
            "openai",
        ));
    }

    #[test]
    fn provider_allowed_value_matches_exact_identifiers_only() {
        assert!(provider_matches_allowed_value(
            "claude",
            "provider-1",
            "Claude",
            "custom",
        ));
        assert!(provider_matches_allowed_value(
            "CLAUDE",
            "provider-1",
            "Claude",
            "custom",
        ));
        assert!(provider_matches_allowed_value(
            "provider-1",
            "provider-1",
            "Other",
            "claude",
        ));
        assert!(!provider_matches_allowed_value(
            "vendor-x",
            "provider-1",
            "Other",
            "claude",
        ));
        assert!(!provider_matches_allowed_value(
            "claude",
            "provider-1",
            "OtherVendor",
            "custom",
        ));
        assert!(!provider_matches_allowed_value(
            "provider-1:extra",
            "provider-1",
            "Other",
            "claude",
        ));
        assert!(!provider_matches_allowed_value(
            "openai:responses",
            "provider-1",
            "Other",
            "claude",
        ));
    }

    #[test]
    fn constraints_normalize_api_formats_and_models() {
        let constraints = sample_constraints();
        assert!(auth_constraints_allow_api_format(
            Some(&constraints),
            "openai:chat"
        ));
        assert!(auth_constraints_allow_model(
            Some(&constraints),
            "gpt-5",
            "gpt-5"
        ));
        assert!(!auth_constraints_allow_model(
            Some(&constraints),
            "gpt-4.1",
            "gpt-4.1"
        ));
    }

    #[test]
    fn api_format_allowed_value_matches_current_signatures_only() {
        assert!(api_format_matches_allowed_value(
            "CLAUDE:MESSAGES",
            "claude:messages"
        ));
        assert!(api_format_matches_allowed_value(
            "openai:responses",
            "openai:responses"
        ));
        assert!(!api_format_matches_allowed_value(
            "openai:responses",
            "claude:messages"
        ));
        assert!(!api_format_matches_allowed_value(
            "claude:messages:extra",
            "claude:messages"
        ));
    }
}
