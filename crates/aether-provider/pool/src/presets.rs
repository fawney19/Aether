use std::collections::BTreeSet;

use aether_pool_core::PoolSchedulingPreset;
use serde_json::{json, Value};

use crate::capability::ProviderPoolCapability;
use crate::provider::ProviderPoolAdapter;
use crate::service::ProviderPoolService;

pub fn normalize_provider_scheduling_presets(
    adapter: &dyn ProviderPoolAdapter,
    scheduling_presets: &[PoolSchedulingPreset],
) -> Vec<PoolSchedulingPreset> {
    let mut entries = Vec::<(usize, PoolSchedulingPreset)>::new();
    let mut seen = BTreeSet::new();

    for (index, item) in scheduling_presets.iter().enumerate() {
        let preset = item.preset.trim().to_ascii_lowercase();
        if preset.is_empty() || !seen.insert(preset.clone()) {
            continue;
        }
        if !provider_pool_supports_preset(adapter, &preset) {
            continue;
        }
        entries.push((
            index,
            PoolSchedulingPreset {
                preset,
                enabled: item.enabled,
                mode: item.mode.clone(),
            },
        ));
    }

    if !entries.is_empty() {
        for preset in adapter.default_scheduling_presets() {
            let preset_name = preset.preset.trim().to_ascii_lowercase();
            if preset_name.is_empty() || seen.contains(&preset_name) {
                continue;
            }
            if !provider_pool_supports_preset(adapter, &preset_name) {
                continue;
            }
            seen.insert(preset_name.clone());
            entries.push((
                entries.len(),
                PoolSchedulingPreset {
                    preset: preset_name,
                    enabled: preset.enabled,
                    mode: preset.mode,
                },
            ));
        }
    }

    let mut distribution_mode = None::<(usize, PoolSchedulingPreset)>;
    let mut strategy_presets = Vec::<(usize, PoolSchedulingPreset)>::new();

    for (index, preset) in entries {
        if !preset.enabled {
            continue;
        }
        if let Some(mutex_group) = provider_pool_preset_mutex_group(&preset.preset) {
            if mutex_group == "distribution_mode"
                && distribution_mode
                    .as_ref()
                    .is_none_or(|current| index < current.0)
            {
                distribution_mode = Some((index, preset));
            }
        } else {
            strategy_presets.push((index, preset));
        }
    }

    let mut normalized = Vec::new();
    if let Some((_, preset)) = distribution_mode.filter(|(_, preset)| preset.preset != "lru") {
        normalized.push(preset);
    }

    strategy_presets.sort_by_key(|left| left.0);
    normalized.extend(strategy_presets.into_iter().map(|(_, preset)| preset));
    normalized
}

pub fn build_admin_pool_scheduling_presets_payload() -> Value {
    let service = ProviderPoolService::with_builtin_adapters();
    json!([
        provider_pool_preset_payload(
            "lru",
            "LRU 轮转",
            "最久未使用的 Key 优先",
            None,
            "依据 LRU 时间戳（最近未使用优先）",
            &service,
        ),
        provider_pool_preset_payload(
            "cache_affinity",
            "缓存亲和",
            "优先复用最近使用过的 Key，利用 Prompt Caching",
            None,
            "依据 LRU 时间戳（最近使用优先，与 LRU 轮转相反）",
            &service,
        ),
        provider_pool_preset_payload(
            "cost_first",
            "成本优先",
            "优先选择窗口消耗更低的账号",
            None,
            "依据窗口成本/Token 用量，缺失时回退配额使用率",
            &service,
        ),
        provider_pool_preset_payload(
            "free_first",
            "Free 优先",
            "优先消耗 Free 账号（依赖 plan_type）",
            Some(ProviderPoolCapability::PlanTier),
            "依据 plan_type（Free 账号优先调度）",
            &service,
        ),
        provider_pool_preset_payload(
            "health_first",
            "健康优先",
            "优先选择健康分更高、失败更少的账号",
            None,
            "依据 health_by_format 聚合分（含熔断/失败衰减）",
            &service,
        ),
        provider_pool_preset_payload(
            "latency_first",
            "延迟优先",
            "优先选择最近延迟更低的账号",
            None,
            "依据号池延迟窗口均值（latency_window_seconds）",
            &service,
        ),
        provider_pool_preset_payload(
            "load_balance",
            "负载均衡",
            "随机分散 Key 使用，均匀分摊负载",
            None,
            "每次随机分值，实现完全均匀分散",
            &service,
        ),
        provider_pool_preset_payload(
            "plus_first",
            "Plus 优先",
            "优先消耗 Plus 账号（依赖 plan_type）",
            Some(ProviderPoolCapability::PlanTier),
            "依据 plan_type（Plus 账号优先调度）",
            &service,
        ),
        provider_pool_preset_payload(
            "pro_first",
            "Pro 优先",
            "优先消耗 Pro 账号（依赖 plan_type）",
            Some(ProviderPoolCapability::PlanTier),
            "依据 plan_type（Pro 账号优先调度）",
            &service,
        ),
        provider_pool_preset_payload(
            "priority_first",
            "优先级优先",
            "按账号优先级顺序调度（数字越小越优先）",
            None,
            "依据 internal_priority（支持拖拽/手工编辑）",
            &service,
        ),
        provider_pool_preset_payload(
            "quota_balanced",
            "额度平均",
            "优先选额度消耗最少的账号",
            None,
            "依据账号配额使用率；无配额时回退到窗口成本使用",
            &service,
        ),
        provider_pool_preset_payload(
            "recent_refresh",
            "额度刷新优先",
            "优先选即将刷新额度的账号",
            Some(ProviderPoolCapability::QuotaReset),
            "依据账号额度重置倒计时（next_reset / reset_seconds）",
            &service,
        ),
        provider_pool_preset_payload(
            "single_account",
            "单号优先",
            "集中使用同一账号（反向 LRU）",
            None,
            "先按账号优先级（internal_priority），同级再按反向 LRU 集中",
            &service,
        ),
        provider_pool_preset_payload(
            "team_first",
            "Team 优先",
            "优先消耗 Team 账号（依赖 plan_type）",
            Some(ProviderPoolCapability::PlanTier),
            "依据 plan_type（Team 账号优先调度）",
            &service,
        ),
    ])
}

fn provider_pool_preset_payload(
    name: &'static str,
    label: &'static str,
    description: &'static str,
    capability: Option<ProviderPoolCapability>,
    evidence_hint: &'static str,
    service: &ProviderPoolService,
) -> Value {
    let providers = capability
        .map(|capability| service.provider_types_for_capability(capability))
        .unwrap_or_default();
    json!({
        "name": name,
        "label": label,
        "description": description,
        "providers": providers,
        "modes": Value::Null,
        "default_mode": Value::Null,
        "mutex_group": provider_pool_preset_mutex_group(name),
        "evidence_hint": evidence_hint,
    })
}

fn provider_pool_supports_preset(adapter: &dyn ProviderPoolAdapter, preset: &str) -> bool {
    match preset {
        "free_first" | "plus_first" | "pro_first" | "team_first" => adapter
            .capabilities()
            .supports(ProviderPoolCapability::PlanTier),
        "recent_refresh" => adapter
            .capabilities()
            .supports(ProviderPoolCapability::QuotaReset),
        _ => true,
    }
}

fn provider_pool_preset_mutex_group(preset: &str) -> Option<&'static str> {
    match preset {
        "lru" | "cache_affinity" | "load_balance" | "single_account" => Some("distribution_mode"),
        _ => None,
    }
}
