use super::types::SchedulerMinimalCandidateSelectionCandidate;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RequiredCapabilityDescriptor<'a> {
    pub(crate) name: &'a str,
    pub(crate) compatible: bool,
}

pub fn candidate_supports_required_capability(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    required_capability: &str,
) -> bool {
    let required_capability = required_capability.trim();
    if required_capability.is_empty() {
        return true;
    }
    let Some(capabilities) = candidate.key_capabilities.as_ref() else {
        return false;
    };

    if let Some(object) = capabilities.as_object() {
        return object.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case(required_capability)
                && match value {
                    serde_json::Value::Bool(value) => *value,
                    serde_json::Value::String(value) => value.eq_ignore_ascii_case("true"),
                    serde_json::Value::Number(value) => {
                        value.as_i64().is_some_and(|value| value > 0)
                    }
                    _ => false,
                }
        });
    }

    if let Some(items) = capabilities.as_array() {
        return items.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(required_capability))
        });
    }

    false
}

pub fn requested_capability_priority_for_candidate(
    required_capabilities: Option<&serde_json::Value>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> (u32, u32) {
    let Some(required_capabilities) = required_capabilities.and_then(serde_json::Value::as_object)
    else {
        return (0, 0);
    };

    requested_capability_priority_for_candidate_descriptors(
        required_capabilities
            .iter()
            .filter_map(|(capability, value)| {
                requested_capability_is_enabled(value).then_some(RequiredCapabilityDescriptor {
                    name: capability.as_str(),
                    compatible: requested_capability_is_compatible(capability),
                })
            }),
        candidate,
    )
}

fn requested_capability_priority_for_candidate_descriptors<'a, I>(
    required_capabilities: I,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> (u32, u32)
where
    I: IntoIterator<Item = RequiredCapabilityDescriptor<'a>>,
{
    let mut exclusive_misses = 0u32;
    let mut compatible_misses = 0u32;
    for capability in required_capabilities {
        if candidate_supports_required_capability(candidate, capability.name) {
            continue;
        }
        if capability.compatible {
            compatible_misses += 1;
        } else {
            exclusive_misses += 1;
        }
    }

    (exclusive_misses, compatible_misses)
}

fn requested_capability_is_enabled(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => value.eq_ignore_ascii_case("true"),
        serde_json::Value::Number(value) => value.as_i64().is_some_and(|value| value > 0),
        _ => false,
    }
}

fn requested_capability_is_compatible(capability: &str) -> bool {
    matches!(
        capability.trim().to_ascii_lowercase().as_str(),
        "cache_1h" | "context_1m"
    )
}
