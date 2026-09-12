use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const ROUTING_PRIORITY_UNSPECIFIED: i32 = i32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Provider,
    PoolGroup,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankingOverlay {
    #[serde(default)]
    pub allowed_providers: Vec<String>,
    #[serde(default)]
    pub allowed_keys: Vec<String>,
    #[serde(default)]
    pub provider_priority_overrides: BTreeMap<String, i32>,
    #[serde(default)]
    pub key_priority_overrides: BTreeMap<String, i32>,
    /// `api_format -> key_id -> priority`; see `RoutingModelPolicy`.
    #[serde(default)]
    pub key_priority_overrides_by_format: BTreeMap<String, BTreeMap<String, i32>>,
    #[serde(default)]
    pub pool_priority_overrides: BTreeMap<String, i32>,
}

impl RankingOverlay {
    pub fn provider_priority(&self, provider_id: &str, fallback: i32) -> i32 {
        self.provider_priority_overrides
            .get(provider_id)
            .copied()
            .unwrap_or(fallback)
    }

    pub fn key_priority(&self, key_id: &str, fallback: i32) -> i32 {
        self.key_priority_overrides
            .get(key_id)
            .copied()
            .unwrap_or(fallback)
    }

    /// Format-scoped key priority: a per-format override wins, then the
    /// format-agnostic key override, then `fallback`.
    pub fn key_priority_for_format(&self, key_id: &str, api_format: &str, fallback: i32) -> i32 {
        self.key_priority_override_for_format(key_id, api_format)
            .unwrap_or_else(|| self.key_priority(key_id, fallback))
    }

    /// Format-scoped key override using exact (case-insensitive) format match.
    pub fn key_priority_override_for_format(&self, key_id: &str, api_format: &str) -> Option<i32> {
        let api_format = api_format.trim();
        self.key_priority_override_matching_format(key_id, |format| {
            format.trim().eq_ignore_ascii_case(api_format)
        })
    }

    /// Format-scoped key override where the caller decides how configured
    /// format names match the candidate format (for alias-aware matching).
    pub fn key_priority_override_matching_format(
        &self,
        key_id: &str,
        mut format_matches: impl FnMut(&str) -> bool,
    ) -> Option<i32> {
        self.key_priority_overrides_by_format
            .iter()
            .find(|(format, _)| format_matches(format))
            .and_then(|(_, overrides)| overrides.get(key_id).copied())
    }

    pub fn insert_key_priority_override_for_format(
        &mut self,
        api_format: &str,
        key_id: String,
        priority: i32,
    ) {
        self.key_priority_overrides_by_format
            .entry(api_format.trim().to_ascii_lowercase())
            .or_default()
            .insert(key_id, priority);
    }

    pub fn pool_priority(&self, provider_id: &str, fallback: i32) -> i32 {
        self.pool_priority_overrides
            .get(provider_id)
            .copied()
            .unwrap_or(fallback)
    }

    pub fn provider_priority_or_unspecified(&self, provider_id: &str) -> i32 {
        self.provider_priority_overrides
            .get(provider_id)
            .copied()
            .unwrap_or(ROUTING_PRIORITY_UNSPECIFIED)
    }

    pub fn key_priority_or_unspecified(&self, key_id: &str) -> i32 {
        self.key_priority_overrides
            .get(key_id)
            .copied()
            .unwrap_or(ROUTING_PRIORITY_UNSPECIFIED)
    }

    pub fn pool_priority_or_unspecified(&self, provider_id: &str) -> i32 {
        self.pool_priority_overrides
            .get(provider_id)
            .copied()
            .unwrap_or(ROUTING_PRIORITY_UNSPECIFIED)
    }

    pub fn provider_allowed(&self, provider_id: &str) -> bool {
        self.allowed_providers.is_empty()
            || self
                .allowed_providers
                .iter()
                .any(|item| item == provider_id)
    }

    pub fn key_allowed(&self, key_id: &str) -> bool {
        self.allowed_keys.is_empty() || self.allowed_keys.iter().any(|item| item == key_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingCandidateFacts {
    pub candidate_kind: CandidateKind,
    pub provider_id: String,
    pub endpoint_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Candidate API format used to resolve format-scoped key overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_format: Option<String>,
    pub provider_priority: i32,
    pub key_priority: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingCandidateRankVector {
    pub provider_priority_before: i32,
    pub provider_priority_after: i32,
    pub key_priority_before: i32,
    pub key_priority_after: i32,
}

pub fn rank_vector_for_candidate(
    overlay: &RankingOverlay,
    facts: &RoutingCandidateFacts,
) -> RoutingCandidateRankVector {
    RoutingCandidateRankVector {
        provider_priority_before: facts.provider_priority,
        provider_priority_after: overlay
            .provider_priority(&facts.provider_id, facts.provider_priority),
        key_priority_before: facts.key_priority,
        key_priority_after: match facts.candidate_kind {
            CandidateKind::Provider => facts
                .key_id
                .as_deref()
                .map(|key_id| match facts.api_format.as_deref() {
                    Some(api_format) => {
                        overlay.key_priority_for_format(key_id, api_format, facts.key_priority)
                    }
                    None => overlay.key_priority(key_id, facts.key_priority),
                })
                .unwrap_or(facts.key_priority),
            CandidateKind::PoolGroup => {
                overlay.pool_priority(&facts.provider_id, facts.key_priority)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn overlay_applies_provider_and_key_priority() {
        let overlay = RankingOverlay {
            provider_priority_overrides: BTreeMap::from([("provider-a".to_string(), 2)]),
            key_priority_overrides: BTreeMap::from([("key-a".to_string(), 5)]),
            ..RankingOverlay::default()
        };
        let facts = RoutingCandidateFacts {
            candidate_kind: CandidateKind::Provider,
            provider_id: "provider-a".to_string(),
            endpoint_id: "endpoint-a".to_string(),
            model_id: "model-a".to_string(),
            key_id: Some("key-a".to_string()),
            api_format: None,
            provider_priority: 10,
            key_priority: 20,
        };

        let vector = rank_vector_for_candidate(&overlay, &facts);
        assert_eq!(vector.provider_priority_after, 2);
        assert_eq!(vector.key_priority_after, 5);
    }

    #[test]
    fn format_scoped_key_override_wins_over_key_override_for_matching_format() {
        let mut overlay = RankingOverlay {
            key_priority_overrides: BTreeMap::from([("key-a".to_string(), 5)]),
            ..RankingOverlay::default()
        };
        overlay.insert_key_priority_override_for_format("openai:chat", "key-a".to_string(), 1);

        assert_eq!(
            overlay.key_priority_for_format("key-a", "openai:chat", 20),
            1
        );
        assert_eq!(
            overlay.key_priority_for_format("key-a", "OpenAI:Chat", 20),
            1
        );
        assert_eq!(
            overlay.key_priority_for_format("key-a", "claude:messages", 20),
            5
        );
        assert_eq!(
            overlay.key_priority_for_format("key-b", "openai:chat", 20),
            20
        );

        let facts = RoutingCandidateFacts {
            candidate_kind: CandidateKind::Provider,
            provider_id: "provider-a".to_string(),
            endpoint_id: "endpoint-a".to_string(),
            model_id: "model-a".to_string(),
            key_id: Some("key-a".to_string()),
            api_format: Some("openai:chat".to_string()),
            provider_priority: 10,
            key_priority: 20,
        };
        assert_eq!(
            rank_vector_for_candidate(&overlay, &facts).key_priority_after,
            1
        );
    }

    #[test]
    fn rank_vector_falls_back_to_existing_priorities() {
        let facts = RoutingCandidateFacts {
            candidate_kind: CandidateKind::Provider,
            provider_id: "provider-a".to_string(),
            endpoint_id: "endpoint-a".to_string(),
            model_id: "model-a".to_string(),
            key_id: Some("key-a".to_string()),
            api_format: None,
            provider_priority: 10,
            key_priority: 20,
        };

        let vector = rank_vector_for_candidate(&RankingOverlay::default(), &facts);
        assert_eq!(vector.provider_priority_after, 10);
        assert_eq!(vector.key_priority_after, 20);
    }

    #[test]
    fn rank_vector_uses_pool_priority_for_pool_groups() {
        let overlay = RankingOverlay {
            pool_priority_overrides: BTreeMap::from([("provider-a".to_string(), 4)]),
            ..RankingOverlay::default()
        };
        let facts = RoutingCandidateFacts {
            candidate_kind: CandidateKind::PoolGroup,
            provider_id: "provider-a".to_string(),
            endpoint_id: "endpoint-a".to_string(),
            model_id: "model-a".to_string(),
            key_id: None,
            api_format: None,
            provider_priority: 10,
            key_priority: 20,
        };

        let vector = rank_vector_for_candidate(&overlay, &facts);

        assert_eq!(vector.key_priority_after, 4);
    }
}
