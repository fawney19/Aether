use super::config::{
    CodexAdapterRuntimeCandidate, CodexAdapterRuntimeRoute, CodexAdapterSchedulingMode,
};

pub(crate) struct CodexAdapterSelectionInput<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) api_key_id: &'a str,
    pub(crate) codex_model: &'a str,
    pub(crate) trace_id: &'a str,
    pub(crate) session_key: Option<&'a str>,
}

pub(crate) fn ordered_global_models_for_route(
    route: &CodexAdapterRuntimeRoute,
    input: CodexAdapterSelectionInput<'_>,
) -> Vec<String> {
    let enabled_candidates = route
        .candidates
        .iter()
        .filter(|candidate| candidate.enabled)
        .collect::<Vec<_>>();
    match route.scheduling_mode {
        CodexAdapterSchedulingMode::Priority => priority_ordered_models(&enabled_candidates),
        CodexAdapterSchedulingMode::Sticky => match input.session_key {
            Some(session_key) => {
                let hash = stable_hash([
                    input.user_id,
                    input.api_key_id,
                    input.codex_model,
                    session_key,
                ]);
                weighted_first_with_priority_tail(&enabled_candidates, hash)
            }
            None => priority_ordered_models(&enabled_candidates),
        },
        CodexAdapterSchedulingMode::LoadBalance => {
            let hash = stable_hash([input.api_key_id, input.codex_model, input.trace_id]);
            weighted_first_with_priority_tail(&enabled_candidates, hash)
        }
    }
}

fn priority_ordered_models(candidates: &[&CodexAdapterRuntimeCandidate]) -> Vec<String> {
    let mut ordered = candidates.to_vec();
    ordered.sort_by_key(|candidate| (candidate.priority, candidate.order));
    ordered
        .into_iter()
        .map(|candidate| candidate.global_model.clone())
        .collect()
}

fn weighted_first_with_priority_tail(
    candidates: &[&CodexAdapterRuntimeCandidate],
    hash: u64,
) -> Vec<String> {
    let Some(first) = weighted_candidate(candidates, hash) else {
        return Vec::new();
    };
    let mut models = vec![first.global_model.clone()];
    let mut tail = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.order != first.order)
        .collect::<Vec<_>>();
    tail.sort_by_key(|candidate| (candidate.priority, candidate.order));
    models.extend(
        tail.into_iter()
            .map(|candidate| candidate.global_model.clone()),
    );
    models
}

fn weighted_candidate<'a>(
    candidates: &[&'a CodexAdapterRuntimeCandidate],
    hash: u64,
) -> Option<&'a CodexAdapterRuntimeCandidate> {
    let total_weight = candidates
        .iter()
        .map(|candidate| u64::from(candidate.weight))
        .sum::<u64>();
    if total_weight == 0 {
        return None;
    }
    let mut slot = hash % total_weight;
    for candidate in candidates {
        let weight = u64::from(candidate.weight);
        if slot < weight {
            return Some(*candidate);
        }
        slot = slot.saturating_sub(weight);
    }
    candidates.last().copied()
}

fn stable_hash<const N: usize>(parts: [&str; N]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::super::config::{
        CodexAdapterRuntimeCandidate, CodexAdapterRuntimeRoute, CodexAdapterSchedulingMode,
    };
    use super::{ordered_global_models_for_route, CodexAdapterSelectionInput};

    fn sample_route(scheduling_mode: CodexAdapterSchedulingMode) -> CodexAdapterRuntimeRoute {
        CodexAdapterRuntimeRoute {
            codex_model: "gpt-5.5".to_string(),
            enabled: true,
            scheduling_mode,
            candidates: vec![
                CodexAdapterRuntimeCandidate {
                    global_model: "glm-4.6".to_string(),
                    enabled: true,
                    priority: 0,
                    weight: 70,
                    order: 0,
                },
                CodexAdapterRuntimeCandidate {
                    global_model: "deepseek-v3.2".to_string(),
                    enabled: true,
                    priority: 1,
                    weight: 30,
                    order: 1,
                },
                CodexAdapterRuntimeCandidate {
                    global_model: "disabled-model".to_string(),
                    enabled: false,
                    priority: -10,
                    weight: 100,
                    order: 2,
                },
            ],
        }
    }

    fn sample_input(
        trace_id: &'static str,
        session_key: Option<&'static str>,
    ) -> CodexAdapterSelectionInput<'static> {
        CodexAdapterSelectionInput {
            user_id: "user-1",
            api_key_id: "api-key-1",
            codex_model: "gpt-5.5",
            trace_id,
            session_key,
        }
    }

    #[test]
    fn priority_orders_by_priority_then_config_order() {
        let route = sample_route(CodexAdapterSchedulingMode::Priority);

        let selected = ordered_global_models_for_route(&route, sample_input("trace-1", None));

        assert_eq!(
            selected,
            vec!["glm-4.6".to_string(), "deepseek-v3.2".to_string()]
        );
    }

    #[test]
    fn sticky_uses_session_and_degrades_to_priority_without_session() {
        let route = sample_route(CodexAdapterSchedulingMode::Sticky);

        let sticky_selected =
            ordered_global_models_for_route(&route, sample_input("trace-1", Some("session-a")));
        let without_session =
            ordered_global_models_for_route(&route, sample_input("trace-1", None));

        assert_eq!(
            without_session,
            vec!["glm-4.6".to_string(), "deepseek-v3.2".to_string()]
        );
        assert_eq!(sticky_selected.len(), 2);
        assert!(sticky_selected.contains(&"glm-4.6".to_string()));
        assert!(sticky_selected.contains(&"deepseek-v3.2".to_string()));
    }

    #[test]
    fn load_balance_uses_weighted_hash_for_first_model_and_keeps_failover_tail() {
        let route = sample_route(CodexAdapterSchedulingMode::LoadBalance);

        let selected = ordered_global_models_for_route(&route, sample_input("trace-1", None));

        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&"glm-4.6".to_string()));
        assert!(selected.contains(&"deepseek-v3.2".to_string()));
    }
}
