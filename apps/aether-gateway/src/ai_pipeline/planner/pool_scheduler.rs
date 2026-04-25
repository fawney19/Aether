use std::cmp::Ordering;
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::{Map, Value};
use tracing::warn;

use crate::ai_pipeline::planner::candidate_eligibility::{
    EligibleLocalExecutionCandidate, SkippedLocalExecutionCandidate,
};
use crate::ai_pipeline::PlannerAppState;
use crate::clock::current_unix_ms;
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::handlers::shared::provider_pool::read_admin_provider_pool_runtime_state;
use crate::handlers::shared::provider_pool::{
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState, AdminProviderPoolSchedulingPreset,
};
use crate::handlers::shared::{
    parse_catalog_auth_config_json, provider_key_health_summary,
    provider_key_status_snapshot_payload,
};
use crate::orchestration::LocalExecutionCandidateMetadata;
use crate::provider_key_auth::provider_key_auth_semantics;

const POOL_ACCOUNT_BLOCKED_SKIP_REASON: &str = "pool_account_blocked";
const POOL_ACCOUNT_EXHAUSTED_SKIP_REASON: &str = "pool_account_exhausted";
const POOL_COOLDOWN_SKIP_REASON: &str = "pool_cooldown";
const POOL_COST_LIMIT_REACHED_SKIP_REASON: &str = "pool_cost_limit_reached";
static LOAD_BALANCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PoolGroupKey {
    provider_id: String,
    endpoint_id: String,
    model_id: String,
    selected_provider_model_name: String,
    provider_api_format: String,
    singleton_key_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct PoolCatalogKeyContext {
    oauth_plan_type: Option<String>,
    quota_usage_ratio: Option<f64>,
    quota_reset_seconds: Option<f64>,
    account_blocked: bool,
    quota_exhausted: bool,
    health_score: Option<f64>,
    latency_avg_ms: Option<f64>,
    catalog_lru_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedPoolPreset {
    preset: String,
    mode: Option<String>,
}

pub(crate) async fn apply_local_execution_pool_scheduler(
    state: PlannerAppState<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    sticky_session_token: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    if candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let sticky_session_token = sticky_session_token
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let mut provider_runtime_requirements =
        BTreeMap::<String, (AdminProviderPoolConfig, BTreeSet<String>)>::new();
    for candidate in &candidates {
        let Some(pool_config) = pool_config_for_candidate(candidate) else {
            continue;
        };
        let entry = provider_runtime_requirements
            .entry(candidate.candidate.provider_id.clone())
            .or_insert_with(|| (pool_config.clone(), BTreeSet::new()));
        entry.1.insert(candidate.candidate.key_id.clone());
    }

    let key_context_by_id = read_pool_catalog_key_contexts_by_id(state, &candidates).await;

    let mut runtime_by_provider = BTreeMap::new();
    let redis_runner = state.app().redis_kv_runner();
    for (provider_id, (pool_config, key_ids)) in provider_runtime_requirements {
        let key_ids = key_ids.into_iter().collect::<Vec<_>>();
        let runtime = match redis_runner.as_ref() {
            Some(runner) if !key_ids.is_empty() => {
                read_admin_provider_pool_runtime_state(
                    runner,
                    provider_id.as_str(),
                    &key_ids,
                    &pool_config,
                    sticky_session_token,
                )
                .await
            }
            _ => AdminProviderPoolRuntimeState::default(),
        };
        runtime_by_provider.insert(provider_id, runtime);
    }

    apply_local_execution_pool_scheduler_with_runtime_map(
        candidates,
        &runtime_by_provider,
        &key_context_by_id,
    )
}

async fn read_pool_catalog_key_contexts_by_id(
    state: PlannerAppState<'_>,
    candidates: &[EligibleLocalExecutionCandidate],
) -> BTreeMap<String, PoolCatalogKeyContext> {
    let mut key_ids = Vec::new();
    let mut provider_type_by_key_id = BTreeMap::<String, String>::new();

    for candidate in candidates {
        if pool_config_for_candidate(candidate).is_none() {
            continue;
        }
        let key_id = candidate.candidate.key_id.clone();
        if let Entry::Vacant(entry) = provider_type_by_key_id.entry(key_id.clone()) {
            entry.insert(candidate.transport.provider.provider_type.clone());
            key_ids.push(key_id);
        }
    }

    if key_ids.is_empty() {
        return BTreeMap::new();
    }

    let keys = match state
        .app()
        .read_provider_catalog_keys_by_ids(&key_ids)
        .await
    {
        Ok(keys) => keys,
        Err(err) => {
            warn!(
                error = ?err,
                key_count = key_ids.len(),
                "gateway pool scheduler: failed to read catalog key metadata"
            );
            return BTreeMap::new();
        }
    };

    keys.into_iter()
        .map(|key| {
            let provider_type = provider_type_by_key_id
                .get(&key.id)
                .map(String::as_str)
                .unwrap_or_default();
            (
                key.id.clone(),
                build_pool_catalog_key_context(state, &key, provider_type),
            )
        })
        .collect()
}

fn build_pool_catalog_key_context(
    state: PlannerAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> PoolCatalogKeyContext {
    let status_snapshot = provider_key_status_snapshot_payload(key, provider_type);
    let quota_snapshot = status_snapshot
        .as_object()
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object);
    let account_snapshot = status_snapshot
        .as_object()
        .and_then(|snapshot| snapshot.get("account"))
        .and_then(Value::as_object);

    let (health_score, _, _, _, _) = provider_key_health_summary(key);
    let health_score = key
        .health_by_format
        .as_ref()
        .and_then(Value::as_object)
        .filter(|payload| !payload.is_empty())
        .map(|_| health_score);
    let latency_avg_ms = key
        .success_count
        .filter(|count| *count > 0)
        .zip(key.total_response_time_ms)
        .map(|(success_count, total_response_time_ms)| {
            f64::from(total_response_time_ms) / f64::from(success_count)
        })
        .filter(|value| value.is_finite() && *value >= 0.0);

    PoolCatalogKeyContext {
        oauth_plan_type: quota_snapshot
            .and_then(|quota| quota.get("plan_type"))
            .and_then(Value::as_str)
            .and_then(|value| normalize_pool_plan_type(value, provider_type))
            .or_else(|| derive_pool_oauth_plan_type(state, key, provider_type)),
        quota_usage_ratio: quota_snapshot
            .and_then(|quota| quota.get("usage_ratio"))
            .and_then(json_f64)
            .map(|value| value.clamp(0.0, 1.0)),
        quota_reset_seconds: quota_snapshot
            .and_then(|quota| quota.get("reset_seconds"))
            .and_then(json_f64)
            .filter(|value| *value >= 0.0),
        account_blocked: account_snapshot
            .and_then(|account| account.get("blocked"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        quota_exhausted: quota_snapshot
            .and_then(|quota| quota.get("exhausted"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        health_score,
        latency_avg_ms,
        catalog_lru_score: Some(key.last_used_at_unix_secs.unwrap_or(0) as f64),
    }
}

fn derive_pool_oauth_plan_type(
    state: PlannerAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<String> {
    if !provider_key_auth_semantics(key, provider_type).oauth_managed() {
        return None;
    }

    let provider_type_key = provider_type.trim().to_ascii_lowercase();
    if let Some(upstream_metadata) = key.upstream_metadata.as_ref().and_then(Value::as_object) {
        let provider_bucket = upstream_metadata
            .get(&provider_type_key)
            .and_then(Value::as_object);
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            if let Some(plan_type) = pool_plan_type_from_source(
                source,
                provider_type,
                &[
                    "plan_type",
                    "tier",
                    "subscription_title",
                    "subscription_plan",
                    "plan",
                ],
            ) {
                return Some(plan_type);
            }
        }
    }

    parse_catalog_auth_config_json(state.app(), key).and_then(|auth_config| {
        pool_plan_type_from_source(
            &auth_config,
            provider_type,
            &["plan_type", "tier", "plan", "subscription_plan"],
        )
    })
}

fn pool_plan_type_from_source(
    source: &Map<String, Value>,
    provider_type: &str,
    fields: &[&str],
) -> Option<String> {
    for field in fields {
        let Some(value) = source.get(*field).and_then(Value::as_str) else {
            continue;
        };
        if let Some(normalized) = normalize_pool_plan_type(value, provider_type) {
            return Some(normalized);
        }
    }
    None
}

fn normalize_pool_plan_type(value: &str, provider_type: &str) -> Option<String> {
    let mut normalized = value.trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    let provider_type = provider_type.trim().to_ascii_lowercase();
    if !provider_type.is_empty() && normalized.to_ascii_lowercase().starts_with(&provider_type) {
        normalized = normalized[provider_type.len()..]
            .trim_matches(|ch: char| [' ', ':', '-', '_'].contains(&ch))
            .to_string();
    }

    let normalized = normalized.trim().to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn json_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

fn apply_local_execution_pool_scheduler_with_runtime_map(
    candidates: Vec<EligibleLocalExecutionCandidate>,
    runtime_by_provider: &BTreeMap<String, AdminProviderPoolRuntimeState>,
    key_context_by_id: &BTreeMap<String, PoolCatalogKeyContext>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let mut group_order = Vec::new();
    let mut groups = BTreeMap::<PoolGroupKey, Vec<EligibleLocalExecutionCandidate>>::new();

    for candidate in candidates {
        let pool_enabled = pool_config_for_candidate(&candidate).is_some();
        let group_key = pool_group_key(&candidate, pool_enabled);
        match groups.entry(group_key) {
            Entry::Vacant(entry) => {
                group_order.push(entry.key().clone());
                entry.insert(vec![candidate]);
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().push(candidate);
            }
        }
    }

    let mut reordered = Vec::new();
    let mut skipped = Vec::new();
    let default_runtime = AdminProviderPoolRuntimeState::default();

    for group_key in group_order {
        let Some(group) = groups.remove(&group_key) else {
            continue;
        };
        let candidate_group_id = local_execution_candidate_group_id(&group_key);
        let Some(pool_config) =
            pool_config_for_candidate(group.first().expect("group should exist"))
        else {
            reordered.extend(annotate_local_execution_group_candidates(
                group,
                candidate_group_id.as_str(),
                false,
            ));
            continue;
        };
        let runtime = runtime_by_provider
            .get(&group_key.provider_id)
            .unwrap_or(&default_runtime);
        let (group_candidates, group_skipped) = schedule_pool_group(
            group,
            pool_config,
            runtime,
            key_context_by_id,
            candidate_group_id.as_str(),
        );
        reordered.extend(group_candidates);
        skipped.extend(group_skipped);
    }

    (reordered, skipped)
}

fn pool_group_key(candidate: &EligibleLocalExecutionCandidate, pool_enabled: bool) -> PoolGroupKey {
    PoolGroupKey {
        provider_id: candidate.candidate.provider_id.clone(),
        endpoint_id: candidate.candidate.endpoint_id.clone(),
        model_id: candidate.candidate.model_id.clone(),
        selected_provider_model_name: candidate.candidate.selected_provider_model_name.clone(),
        provider_api_format: candidate.provider_api_format.clone(),
        singleton_key_id: (!pool_enabled).then(|| candidate.candidate.key_id.clone()),
    }
}

fn local_execution_candidate_group_id(group_key: &PoolGroupKey) -> String {
    format!(
        "provider={}|endpoint={}|model={}|selected_model={}|api_format={}|singleton_key={}",
        group_key.provider_id,
        group_key.endpoint_id,
        group_key.model_id,
        group_key.selected_provider_model_name,
        group_key.provider_api_format,
        group_key.singleton_key_id.as_deref().unwrap_or("*"),
    )
}

fn pool_config_for_candidate(
    candidate: &EligibleLocalExecutionCandidate,
) -> Option<AdminProviderPoolConfig> {
    admin_provider_pool_config_from_config_value(candidate.transport.provider.config.as_ref())
}

fn schedule_pool_group(
    group: Vec<EligibleLocalExecutionCandidate>,
    pool_config: AdminProviderPoolConfig,
    runtime: &AdminProviderPoolRuntimeState,
    key_context_by_id: &BTreeMap<String, PoolCatalogKeyContext>,
    candidate_group_id: &str,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let provider_type = group
        .first()
        .map(|candidate| {
            candidate
                .transport
                .provider
                .provider_type
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let active_presets =
        normalize_enabled_pool_presets(&pool_config.scheduling_presets, provider_type.as_str());

    let mut available = Vec::new();
    let mut skipped = Vec::new();

    for (original_index, eligible) in group.into_iter().enumerate() {
        let EligibleLocalExecutionCandidate {
            candidate,
            transport,
            provider_api_format,
            orchestration,
        } = eligible;
        let key_id = candidate.key_id.clone();
        let mut key_context = key_context_by_id.get(&key_id).cloned().unwrap_or_default();
        key_context.latency_avg_ms = runtime
            .latency_avg_ms_by_key
            .get(&key_id)
            .copied()
            .or(key_context.latency_avg_ms);

        if key_context.account_blocked {
            skipped.push(SkippedLocalExecutionCandidate {
                candidate,
                skip_reason: POOL_ACCOUNT_BLOCKED_SKIP_REASON,
                transport: Some(transport),
                extra_data: None,
            });
            continue;
        }

        if pool_config.skip_exhausted_accounts && key_context.quota_exhausted {
            skipped.push(SkippedLocalExecutionCandidate {
                candidate,
                skip_reason: POOL_ACCOUNT_EXHAUSTED_SKIP_REASON,
                transport: Some(transport),
                extra_data: None,
            });
            continue;
        }

        if runtime.cooldown_reason_by_key.contains_key(&key_id) {
            skipped.push(SkippedLocalExecutionCandidate {
                candidate,
                skip_reason: POOL_COOLDOWN_SKIP_REASON,
                transport: Some(transport),
                extra_data: None,
            });
            continue;
        }

        if pool_config
            .cost_limit_per_key_tokens
            .is_some_and(|limit| runtime_cost_usage(runtime, key_id.as_str()) >= limit)
        {
            skipped.push(SkippedLocalExecutionCandidate {
                candidate,
                skip_reason: POOL_COST_LIMIT_REACHED_SKIP_REASON,
                transport: Some(transport),
                extra_data: None,
            });
            continue;
        }

        let lru_score =
            runtime_lru_score(runtime, key_id.as_str()).or(key_context.catalog_lru_score);

        available.push(PoolGroupCandidateOrdering {
            eligible: EligibleLocalExecutionCandidate {
                candidate,
                transport,
                provider_api_format,
                orchestration,
            },
            key_context,
            original_index,
            lru_score,
            cost_usage: runtime_cost_usage(runtime, key_id.as_str()),
        });
    }

    if available.is_empty() {
        return (Vec::new(), skipped);
    }

    let sticky_candidate = runtime
        .sticky_bound_key_id
        .as_ref()
        .and_then(|sticky_key_id| {
            available
                .iter()
                .position(|item| item.eligible.candidate.key_id == *sticky_key_id)
        })
        .map(|index| available.remove(index));

    if !active_presets.is_empty() {
        let sort_vectors = build_pool_sort_vectors(
            &available,
            &active_presets,
            pool_config.lru_enabled,
            group_sort_seed(
                provider_type.as_str(),
                available.first().map(|item| &item.eligible.candidate),
            )
            .as_str(),
            pool_config.cost_limit_per_key_tokens,
        );
        available.sort_by(|left, right| {
            sort_vectors
                .get(&left.eligible.candidate.key_id)
                .cmp(&sort_vectors.get(&right.eligible.candidate.key_id))
                .then(left.original_index.cmp(&right.original_index))
        });
    } else if pool_config.lru_enabled {
        let lru_ranks = lru_rank_indices(&available, false);
        available.sort_by(|left, right| {
            lru_ranks
                .get(&left.eligible.candidate.key_id)
                .cmp(&lru_ranks.get(&right.eligible.candidate.key_id))
                .then(left.original_index.cmp(&right.original_index))
        });
    }

    let mut ordered = Vec::new();
    if let Some(sticky_candidate) = sticky_candidate {
        ordered.push(sticky_candidate.eligible);
    }
    ordered.extend(available.into_iter().map(|item| item.eligible));

    (
        annotate_local_execution_group_candidates(ordered, candidate_group_id, true),
        skipped,
    )
}

fn annotate_local_execution_group_candidates(
    candidates: Vec<EligibleLocalExecutionCandidate>,
    candidate_group_id: &str,
    pool_enabled: bool,
) -> Vec<EligibleLocalExecutionCandidate> {
    candidates
        .into_iter()
        .enumerate()
        .map(|(index, mut candidate)| {
            candidate.orchestration = LocalExecutionCandidateMetadata {
                candidate_group_id: Some(candidate_group_id.to_string()),
                pool_key_index: pool_enabled.then_some(index as u32),
            };
            candidate
        })
        .collect()
}

#[derive(Debug)]
struct PoolGroupCandidateOrdering {
    eligible: EligibleLocalExecutionCandidate,
    key_context: PoolCatalogKeyContext,
    original_index: usize,
    lru_score: Option<f64>,
    cost_usage: u64,
}

fn build_pool_sort_vectors(
    items: &[PoolGroupCandidateOrdering],
    presets: &[NormalizedPoolPreset],
    lru_enabled: bool,
    load_balance_seed: &str,
    cost_limit_per_key_tokens: Option<u64>,
) -> BTreeMap<String, Vec<usize>> {
    let mut vectors = BTreeMap::<String, Vec<usize>>::new();
    let lru_ranks = lru_rank_indices(items, false);
    let cache_affinity_ranks = lru_rank_indices(items, true);

    for preset in presets {
        let ranks = match preset.preset.as_str() {
            "cache_affinity" => cache_affinity_ranks.clone(),
            "priority_first" => priority_first_ranks(items, &lru_ranks),
            "single_account" => single_account_ranks(items),
            "plus_first" => plan_ranks(items, &lru_ranks, Some("plus_only")),
            "pro_first" => plan_ranks(items, &lru_ranks, Some("pro_only")),
            "free_first" => plan_ranks(items, &lru_ranks, Some("free_only")),
            "team_first" => plan_ranks(items, &lru_ranks, Some("team_only")),
            "health_first" => health_first_ranks(items, &lru_ranks),
            "latency_first" => latency_first_ranks(items, &lru_ranks),
            "cost_first" => cost_first_ranks(items, &lru_ranks, cost_limit_per_key_tokens),
            "quota_balanced" => quota_balanced_ranks(items, &lru_ranks, cost_limit_per_key_tokens),
            "recent_refresh" => recent_refresh_ranks(items, &lru_ranks),
            "load_balance" => load_balance_ranks(items, load_balance_seed),
            _ => continue,
        };
        for item in items {
            let key_id = item.eligible.candidate.key_id.clone();
            vectors
                .entry(key_id.clone())
                .or_default()
                .push(*ranks.get(&key_id).unwrap_or(&0));
        }
    }

    if lru_enabled {
        for item in items {
            let key_id = item.eligible.candidate.key_id.clone();
            vectors
                .entry(key_id.clone())
                .or_default()
                .push(*lru_ranks.get(&key_id).unwrap_or(&0));
        }
    }

    vectors
}

fn lru_rank_indices(
    items: &[PoolGroupCandidateOrdering],
    descending: bool,
) -> BTreeMap<String, usize> {
    let scores = collect_metric_scores(items, |item| item.lru_score);
    rank_indices_from_score_map(items, &scores, descending)
}

fn priority_first_ranks(
    items: &[PoolGroupCandidateOrdering],
    lru_ranks: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    let scores = collect_metric_scores(items, |item| {
        Some(f64::from(item.eligible.candidate.key_internal_priority))
    });
    if !score_map_has_variation(&scores) {
        return lru_ranks.clone();
    }
    rank_indices_from_score_map(items, &scores, false)
}

fn single_account_ranks(items: &[PoolGroupCandidateOrdering]) -> BTreeMap<String, usize> {
    let n = items.len().saturating_sub(1).max(1) as f64;
    let priority_scores = collect_metric_scores(items, |item| {
        Some(f64::from(item.eligible.candidate.key_internal_priority))
    });
    let priority_ranks = rank_indices_from_score_map(items, &priority_scores, false);
    let lru_desc_ranks = lru_rank_indices(items, true);
    let combined_scores = items
        .iter()
        .map(|item| {
            let key_id = item.eligible.candidate.key_id.clone();
            let priority_rank = *priority_ranks.get(&key_id).unwrap_or(&0) as f64 / n;
            let lru_rank = *lru_desc_ranks.get(&key_id).unwrap_or(&0) as f64 / n;
            (key_id, Some((priority_rank * 0.75) + (lru_rank * 0.25)))
        })
        .collect::<BTreeMap<_, _>>();
    rank_indices_from_score_map(items, &combined_scores, false)
}

fn plan_ranks(
    items: &[PoolGroupCandidateOrdering],
    lru_ranks: &BTreeMap<String, usize>,
    mode: Option<&str>,
) -> BTreeMap<String, usize> {
    let scores = items
        .iter()
        .map(|item| {
            (
                item.eligible.candidate.key_id.clone(),
                Some(plan_priority_score(
                    item.key_context.oauth_plan_type.as_deref(),
                    mode,
                )),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if !score_map_has_variation(&scores) {
        return lru_ranks.clone();
    }
    rank_indices_from_score_map(items, &scores, false)
}

fn health_first_ranks(
    items: &[PoolGroupCandidateOrdering],
    lru_ranks: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    let scores = collect_metric_scores(items, |item| {
        item.key_context
            .health_score
            .map(|score| 1.0 - score.clamp(0.0, 1.0))
    });
    if !score_map_has_signal(&scores) {
        return lru_ranks.clone();
    }
    rank_indices_from_score_map(items, &scores, false)
}

fn latency_first_ranks(
    items: &[PoolGroupCandidateOrdering],
    lru_ranks: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    let scores = collect_metric_scores(items, |item| item.key_context.latency_avg_ms);
    if !score_map_has_signal(&scores) {
        return lru_ranks.clone();
    }
    rank_indices_from_score_map(items, &scores, false)
}

fn cost_first_ranks(
    items: &[PoolGroupCandidateOrdering],
    lru_ranks: &BTreeMap<String, usize>,
    cost_limit_per_key_tokens: Option<u64>,
) -> BTreeMap<String, usize> {
    let scores = collect_metric_scores(items, |item| {
        cost_penalty(item, cost_limit_per_key_tokens).or(item.key_context.quota_usage_ratio)
    });
    if !score_map_has_signal(&scores) {
        return lru_ranks.clone();
    }
    rank_indices_from_score_map(items, &scores, false)
}

fn quota_balanced_ranks(
    items: &[PoolGroupCandidateOrdering],
    lru_ranks: &BTreeMap<String, usize>,
    cost_limit_per_key_tokens: Option<u64>,
) -> BTreeMap<String, usize> {
    let scores = collect_metric_scores(items, |item| {
        item.key_context
            .quota_usage_ratio
            .or_else(|| cost_penalty(item, cost_limit_per_key_tokens))
    });
    if !score_map_has_signal(&scores) {
        return lru_ranks.clone();
    }
    rank_indices_from_score_map(items, &scores, false)
}

fn recent_refresh_ranks(
    items: &[PoolGroupCandidateOrdering],
    lru_ranks: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    let scores = collect_metric_scores(items, |item| item.key_context.quota_reset_seconds);
    if !score_map_has_signal(&scores) {
        return lru_ranks.clone();
    }
    rank_indices_from_score_map(items, &scores, false)
}

fn load_balance_ranks(
    items: &[PoolGroupCandidateOrdering],
    load_balance_seed: &str,
) -> BTreeMap<String, usize> {
    let scores = items
        .iter()
        .map(|item| {
            let key_id = item.eligible.candidate.key_id.clone();
            (
                key_id.clone(),
                Some(stable_hash_score(
                    format!("{load_balance_seed}:{key_id}").as_str(),
                )),
            )
        })
        .collect::<BTreeMap<_, _>>();
    rank_indices_from_score_map(items, &scores, false)
}

fn group_sort_seed(
    provider_type: &str,
    candidate: Option<&aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate>,
) -> String {
    let now_ms = current_unix_ms();
    let sequence = LOAD_BALANCE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    match candidate {
        Some(candidate) => format!(
            "{provider_type}:{}:{}:{}:{}:{now_ms}:{sequence}",
            candidate.provider_id,
            candidate.endpoint_id,
            candidate.model_id,
            candidate.selected_provider_model_name,
        ),
        None => format!("{provider_type}:{now_ms}:{sequence}"),
    }
}

fn stable_hash_score(seed: &str) -> f64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut hasher);
    let value = hasher.finish();
    value as f64 / u64::MAX as f64
}

fn collect_metric_scores<F>(
    items: &[PoolGroupCandidateOrdering],
    mut score_for: F,
) -> BTreeMap<String, Option<f64>>
where
    F: FnMut(&PoolGroupCandidateOrdering) -> Option<f64>,
{
    items
        .iter()
        .map(|item| (item.eligible.candidate.key_id.clone(), score_for(item)))
        .collect()
}

fn score_map_has_signal(scores: &BTreeMap<String, Option<f64>>) -> bool {
    scores.values().flatten().any(|value| value.is_finite())
}

fn score_map_has_variation(scores: &BTreeMap<String, Option<f64>>) -> bool {
    let mut values = scores
        .values()
        .flatten()
        .filter(|value| value.is_finite())
        .map(|value| value.to_bits())
        .collect::<BTreeSet<_>>();
    values.len() > 1
}

fn rank_indices_from_score_map(
    items: &[PoolGroupCandidateOrdering],
    scores: &BTreeMap<String, Option<f64>>,
    descending: bool,
) -> BTreeMap<String, usize> {
    if !score_map_has_signal(scores) {
        return items
            .iter()
            .map(|item| (item.eligible.candidate.key_id.clone(), 0))
            .collect();
    }

    let mut decorated = items
        .iter()
        .map(|item| {
            let key_id = item.eligible.candidate.key_id.clone();
            let score = scores
                .get(&key_id)
                .copied()
                .flatten()
                .filter(|value| value.is_finite());
            let sortable = score.map(|value| if descending { -value } else { value });
            (
                score.is_none(),
                sortable.unwrap_or(f64::INFINITY),
                item.original_index,
                key_id,
            )
        })
        .collect::<Vec<_>>();
    decorated.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
            .then(left.2.cmp(&right.2))
    });

    decorated
        .into_iter()
        .enumerate()
        .map(|(rank, (_, _, _, key_id))| (key_id, rank))
        .collect()
}

fn cost_penalty(
    item: &PoolGroupCandidateOrdering,
    cost_limit_per_key_tokens: Option<u64>,
) -> Option<f64> {
    if item.cost_usage == 0 {
        return None;
    }

    if let Some(limit) = cost_limit_per_key_tokens.filter(|limit| *limit > 0) {
        return Some((item.cost_usage as f64 / limit as f64).clamp(0.0, 1.0));
    }

    let used = item.cost_usage as f64;
    Some((used / (used + 10_000.0)).clamp(0.0, 1.0))
}

fn plan_priority_score(plan_type: Option<&str>, mode: Option<&str>) -> f64 {
    match mode.unwrap_or("both").trim().to_ascii_lowercase().as_str() {
        "free_only" => match plan_type {
            Some("free") => 0.0,
            Some("team") => 0.5,
            Some("enterprise" | "business") => 0.2,
            Some("plus" | "pro") => 0.6,
            Some(_) => 0.7,
            None => 0.8,
        },
        "team_only" => match plan_type {
            Some("team") => 0.0,
            Some("free") => 0.5,
            Some("enterprise" | "business") => 0.2,
            Some("plus" | "pro") => 0.6,
            Some(_) => 0.7,
            None => 0.8,
        },
        "plus_only" => match plan_type {
            Some("plus" | "pro") => 0.0,
            Some("enterprise" | "business") => 0.3,
            Some("free" | "team") => 0.7,
            Some(_) => 0.7,
            None => 0.8,
        },
        "pro_only" => match plan_type {
            Some("pro") => 0.0,
            Some("plus") => 0.3,
            Some("enterprise" | "business") => 0.4,
            Some("free" | "team") => 0.7,
            Some(_) => 0.7,
            None => 0.8,
        },
        _ => match plan_type {
            Some("free" | "team") => 0.0,
            Some("enterprise" | "business") => 0.2,
            Some("plus" | "pro") => 0.6,
            Some(_) => 0.7,
            None => 0.8,
        },
    }
}

fn normalize_enabled_pool_presets(
    scheduling_presets: &[AdminProviderPoolSchedulingPreset],
    provider_type: &str,
) -> Vec<NormalizedPoolPreset> {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let mut entries = Vec::<(usize, String, bool, Option<String>)>::new();
    let mut seen = BTreeSet::new();

    for (index, item) in scheduling_presets.iter().enumerate() {
        let preset = item.preset.trim().to_ascii_lowercase();
        if preset.is_empty() || !seen.insert(preset.clone()) {
            continue;
        }
        entries.push((index, preset, item.enabled, item.mode.clone()));
    }

    if provider_type == "codex"
        && !entries.is_empty()
        && entries
            .iter()
            .all(|(_, preset, _, _)| preset != "recent_refresh")
    {
        entries.push((entries.len(), "recent_refresh".to_string(), true, None));
    }

    let mut group_anchor_index = BTreeMap::<String, usize>::new();
    for (index, preset, _, _) in &entries {
        let Some(mutex_group) = pool_preset_mutex_group(preset) else {
            continue;
        };
        group_anchor_index
            .entry(mutex_group.to_string())
            .or_insert(*index);
    }

    let mut ordered_enabled = Vec::<(usize, usize, String, Option<String>)>::new();
    let mut group_enabled = BTreeMap::<String, (usize, usize, String, Option<String>)>::new();

    for (index, preset, enabled, mode) in entries {
        if !enabled
            || preset == "lru"
            || !pool_preset_supported_for_provider(&preset, &provider_type)
        {
            continue;
        }

        let Some(mutex_group) = pool_preset_mutex_group(&preset) else {
            ordered_enabled.push((index, index, preset, mode));
            continue;
        };
        let anchor = group_anchor_index
            .get(mutex_group)
            .copied()
            .unwrap_or(index);
        let existing = group_enabled.get(mutex_group);
        if existing.is_none_or(|current| index < current.1) {
            group_enabled.insert(mutex_group.to_string(), (anchor, index, preset, mode));
        }
    }

    ordered_enabled.extend(group_enabled.into_values());
    ordered_enabled.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    ordered_enabled
        .into_iter()
        .map(|(_, _, preset, mode)| NormalizedPoolPreset { preset, mode })
        .collect()
}

fn pool_preset_supported_for_provider(preset: &str, provider_type: &str) -> bool {
    match preset {
        "free_first" | "plus_first" | "pro_first" | "recent_refresh" | "team_first" => {
            matches!(provider_type, "codex" | "kiro")
        }
        _ => true,
    }
}

fn pool_preset_mutex_group(preset: &str) -> Option<&'static str> {
    match preset {
        "lru" | "cache_affinity" | "load_balance" | "single_account" => Some("distribution_mode"),
        _ => None,
    }
}

fn runtime_lru_score(runtime: &AdminProviderPoolRuntimeState, key_id: &str) -> Option<f64> {
    runtime.lru_score_by_key.get(key_id).copied()
}

fn runtime_cost_usage(runtime: &AdminProviderPoolRuntimeState, key_id: &str) -> u64 {
    runtime
        .cost_window_usage_by_key
        .get(key_id)
        .copied()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_local_execution_pool_scheduler_with_runtime_map, build_pool_catalog_key_context,
        normalize_enabled_pool_presets, PoolCatalogKeyContext,
    };
    use crate::ai_pipeline::planner::candidate_eligibility::EligibleLocalExecutionCandidate;
    use crate::ai_pipeline::PlannerAppState;
    use crate::data::GatewayDataState;
    use crate::handlers::shared::provider_pool::{
        AdminProviderPoolRuntimeState, AdminProviderPoolSchedulingPreset,
    };
    use crate::orchestration::LocalExecutionCandidateMetadata;
    use crate::AppState;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[test]
    fn pool_scheduler_groups_interleaved_candidates_and_reorders_internal_keys() {
        let pool_first = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-a",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let other =
            sample_eligible_candidate("provider-other", "endpoint-2", "key-other", 10, None);
        let pool_second = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-b",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-pool-a".to_string(), 20.0),
                    ("key-pool-b".to_string(), 10.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![pool_first, other, pool_second],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-pool-b", "key-pool-a", "key-other"]
        );
    }

    #[test]
    fn pool_scheduler_uses_catalog_last_used_when_runtime_lru_is_missing() {
        let recent_key = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-recent",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let older_key = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-older",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-recent".to_string(),
                PoolCatalogKeyContext {
                    catalog_lru_score: Some(200.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-older".to_string(),
                PoolCatalogKeyContext {
                    catalog_lru_score: Some(100.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![recent_key, older_key],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-older", "key-recent"]
        );
    }

    #[test]
    fn pool_scheduler_attaches_group_and_pool_metadata_to_ranked_candidates() {
        let pool_first = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-a",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let other =
            sample_eligible_candidate("provider-other", "endpoint-2", "key-other", 10, None);
        let pool_second = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-b",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-pool-a".to_string(), 20.0),
                    ("key-pool-b".to_string(), 10.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![pool_first, other, pool_second],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(reordered.len(), 3);
        assert_eq!(
            reordered[0].orchestration,
            LocalExecutionCandidateMetadata {
                candidate_group_id: Some(
                    "provider=provider-pool|endpoint=endpoint-1|model=model-1|selected_model=gpt-5|api_format=openai:chat|singleton_key=*"
                        .to_string(),
                ),
                pool_key_index: Some(0),
            }
        );
        assert_eq!(reordered[1].orchestration.pool_key_index, Some(1));
        assert_eq!(
            reordered[1].orchestration.candidate_group_id,
            reordered[0].orchestration.candidate_group_id
        );
        assert_eq!(
            reordered[2].orchestration,
            LocalExecutionCandidateMetadata {
                candidate_group_id: Some(
                    "provider=provider-other|endpoint=endpoint-2|model=model-1|selected_model=gpt-5|api_format=openai:chat|singleton_key=key-other"
                        .to_string(),
                ),
                pool_key_index: None,
            }
        );
    }

    #[test]
    fn pool_scheduler_promotes_sticky_hit_before_other_sorted_keys() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "cache_affinity", "enabled": true}]
                }
            })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "cache_affinity", "enabled": true}]
                }
            })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                sticky_bound_key_id: Some("key-a".to_string()),
                lru_score_by_key: BTreeMap::from([
                    ("key-a".to_string(), 50.0),
                    ("key-b".to_string(), 10.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_a, key_b],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-a", "key-b"]
        );
    }

    #[test]
    fn pool_scheduler_promotes_sticky_hit_regardless_distribution_mode() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "quota_balanced", "enabled": true}]
                }
            })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "quota_balanced", "enabled": true}]
                }
            })),
        );

        let runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                sticky_bound_key_id: Some("key-a".to_string()),
                ..AdminProviderPoolRuntimeState::default()
            },
        )]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_b, key_a],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-a", "key-b"]
        );
    }

    #[test]
    fn pool_scheduler_skips_cooldown_and_cost_exhausted_keys() {
        let key_ready = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-ready",
            10,
            Some(json!({
                "pool_advanced": {
                    "cost_limit_per_key_tokens": 100
                }
            })),
        );
        let key_cooldown = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-cooldown",
            10,
            Some(json!({
                "pool_advanced": {
                    "cost_limit_per_key_tokens": 100
                }
            })),
        );
        let key_cost = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-cost",
            10,
            Some(json!({
                "pool_advanced": {
                    "cost_limit_per_key_tokens": 100
                }
            })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                cooldown_reason_by_key: BTreeMap::from([(
                    "key-cooldown".to_string(),
                    "429".to_string(),
                )]),
                cost_window_usage_by_key: BTreeMap::from([("key-cost".to_string(), 100)]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_ready, key_cooldown, key_cost],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-ready"]
        );
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![
                ("key-cooldown", "pool_cooldown"),
                ("key-cost", "pool_cost_limit_reached"),
            ]
        );
    }

    #[test]
    fn pool_scheduler_applies_preset_hard_order_before_lru() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            50,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [
                        {"preset": "priority_first", "enabled": true},
                        {"preset": "cache_affinity", "enabled": true}
                    ]
                }
            })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [
                        {"preset": "priority_first", "enabled": true},
                        {"preset": "cache_affinity", "enabled": true}
                    ]
                }
            })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-a".to_string(), 5.0),
                    ("key-b".to_string(), 100.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_a, key_b],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-b", "key-a"]
        );
    }

    #[test]
    fn pool_scheduler_uses_plan_preset_with_catalog_context() {
        let key_free = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-free",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );
        let key_plus = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-plus",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-free".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("free".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-plus".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("plus".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_free, key_plus],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-plus", "key-free"]
        );
    }

    #[test]
    fn pool_scheduler_plus_first_treats_plus_and_pro_as_top_tier() {
        let key_plus = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-plus",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );
        let key_pro = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pro",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );
        let key_team = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-team",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-plus".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("plus".to_string()),
                    catalog_lru_score: Some(300.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-pro".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("pro".to_string()),
                    catalog_lru_score: Some(100.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-team".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("team".to_string()),
                    catalog_lru_score: Some(50.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_plus, key_pro, key_team],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-plus", "key-pro", "key-team"]
        );
    }

    #[test]
    fn pool_scheduler_supports_pro_first_plan_preset() {
        let key_plus = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-plus",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "pro_first", "enabled": true}]
                }
            })),
        );
        let key_pro = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pro",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "pro_first", "enabled": true}]
                }
            })),
        );
        let key_team = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-team",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "pro_first", "enabled": true}]
                }
            })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-plus".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("plus".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-pro".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("pro".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-team".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("team".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_plus, key_team, key_pro],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-pro", "key-plus", "key-team"]
        );
    }

    #[test]
    fn pool_scheduler_defaults_empty_pool_advanced_to_cache_affinity() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            10,
            Some(json!({ "pool_advanced": {} })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({ "pool_advanced": {} })),
        );

        let runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-a".to_string(), 10.0),
                    ("key-b".to_string(), 200.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        )]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_a, key_b],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-b", "key-a"]
        );
    }

    #[test]
    fn normalizes_distribution_mutex_group_to_first_enabled_member() {
        let presets = normalize_enabled_pool_presets(
            &[
                AdminProviderPoolSchedulingPreset {
                    preset: "lru".to_string(),
                    enabled: false,
                    mode: None,
                },
                AdminProviderPoolSchedulingPreset {
                    preset: "single_account".to_string(),
                    enabled: true,
                    mode: None,
                },
                AdminProviderPoolSchedulingPreset {
                    preset: "cache_affinity".to_string(),
                    enabled: true,
                    mode: None,
                },
                AdminProviderPoolSchedulingPreset {
                    preset: "priority_first".to_string(),
                    enabled: true,
                    mode: None,
                },
            ],
            "openai",
        );

        assert_eq!(
            presets
                .iter()
                .map(|item| item.preset.as_str())
                .collect::<Vec<_>>(),
            vec!["single_account", "priority_first"]
        );
    }

    #[test]
    fn builds_pool_catalog_context_from_status_snapshot_and_auth_config() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            None,
            "secret".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("transport fields should build");
        key.status_snapshot = Some(json!({
            "account": {"blocked": false},
            "quota": {
                "usage_ratio": 0.25,
                "reset_seconds": 3600,
                "exhausted": false,
                "plan_type": "team"
            }
        }));
        key.success_count = Some(4);
        key.total_response_time_ms = Some(200);
        key.last_used_at_unix_secs = Some(1_711_000_123);

        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    Vec::new(),
                    Vec::new(),
                    vec![key.clone()],
                )),
            ));

        let context = build_pool_catalog_key_context(PlannerAppState::new(&app), &key, "codex");

        assert_eq!(context.oauth_plan_type.as_deref(), Some("team"));
        assert_eq!(context.quota_usage_ratio, Some(0.25));
        assert_eq!(context.quota_reset_seconds, Some(3600.0));
        assert_eq!(context.latency_avg_ms, Some(50.0));
        assert_eq!(context.catalog_lru_score, Some(1_711_000_123.0));
    }

    fn sample_eligible_candidate(
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        internal_priority: i32,
        provider_config: Option<serde_json::Value>,
    ) -> EligibleLocalExecutionCandidate {
        EligibleLocalExecutionCandidate {
            candidate: SchedulerMinimalCandidateSelectionCandidate {
                provider_id: provider_id.to_string(),
                provider_name: provider_id.to_string(),
                provider_type: "codex".to_string(),
                provider_priority: 10,
                endpoint_id: endpoint_id.to_string(),
                endpoint_api_format: "openai:chat".to_string(),
                key_id: key_id.to_string(),
                key_name: key_id.to_string(),
                key_auth_type: "api_key".to_string(),
                key_internal_priority: internal_priority,
                key_global_priority_for_format: Some(1),
                key_capabilities: None,
                model_id: "model-1".to_string(),
                global_model_id: "global-model-1".to_string(),
                global_model_name: "gpt-5".to_string(),
                selected_provider_model_name: "gpt-5".to_string(),
                mapping_matched_model: None,
            },
            provider_api_format: "openai:chat".to_string(),
            orchestration: LocalExecutionCandidateMetadata::default(),
            transport: Arc::new(crate::ai_pipeline::GatewayProviderTransportSnapshot {
                provider: GatewayProviderTransportProvider {
                    id: provider_id.to_string(),
                    name: provider_id.to_string(),
                    provider_type: "codex".to_string(),
                    website: None,
                    is_active: true,
                    keep_priority_on_conversion: false,
                    enable_format_conversion: false,
                    concurrent_limit: None,
                    max_retries: None,
                    proxy: None,
                    request_timeout_secs: None,
                    stream_first_byte_timeout_secs: None,
                    config: provider_config,
                },
                endpoint: GatewayProviderTransportEndpoint {
                    id: endpoint_id.to_string(),
                    provider_id: provider_id.to_string(),
                    api_format: "openai:chat".to_string(),
                    api_family: Some("openai".to_string()),
                    endpoint_kind: Some("chat".to_string()),
                    is_active: true,
                    base_url: "https://example.com".to_string(),
                    header_rules: None,
                    body_rules: None,
                    max_retries: None,
                    custom_path: None,
                    config: None,
                    format_acceptance_config: None,
                    proxy: None,
                },
                key: GatewayProviderTransportKey {
                    id: key_id.to_string(),
                    provider_id: provider_id.to_string(),
                    name: key_id.to_string(),
                    auth_type: "api_key".to_string(),
                    is_active: true,
                    api_formats: Some(vec!["openai:chat".to_string()]),
                    allowed_models: None,
                    capabilities: None,
                    rate_multipliers: None,
                    global_priority_by_format: None,
                    expires_at_unix_secs: None,
                    proxy: None,
                    fingerprint: None,
                    decrypted_api_key: "secret".to_string(),
                    decrypted_auth_config: None,
                },
            }),
        }
    }
}
