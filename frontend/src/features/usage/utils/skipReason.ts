/**
 * 候选"被跳过"原因的中文标签。
 *
 * 这些字符串来自后端 `StoredRequestCandidate.skip_reason`，取值受
 * `crates/aether-data/contracts/src/repository/candidates/types.rs` 里的
 * `REQUEST_CANDIDATE_SKIP_REASONS` 白名单约束（未知原因会被后端统一清洗成
 * `unclassified_candidate_skip_reason`）。
 *
 * 维护约定：后端新增白名单项时，这里补一条中文说明；缺失时前端会原样显示英文，
 * 不会丢信息，所以可以安全地"先上线原因、后补翻译"。
 */
export const CANDIDATE_SKIP_REASON_LABELS: Record<string, string> = {
  // —— 调度期运行时可选择性（不满足条件，从未真正发起请求）——
  account_quota_exhausted: '账号额度已耗尽',
  oauth_invalid: 'OAuth 凭证已失效',
  provider_quota_blocked: '上游提供商额度已用尽',
  provider_concurrency_limit_reached: '上游提供商并发已达上限',
  provider_key_concurrency_limit_reached: '上游账号并发已达上限',
  provider_inactive: '提供商已停用',
  key_inactive: '密钥已停用',
  key_circuit_open: '密钥熔断中（连续失败后暂停）',
  key_health_score_zero: '密钥健康分为 0',
  key_rpm_exhausted: '密钥本分钟请求数已达上限',
  key_model_disabled: '该密钥已停用此模型',
  key_model_not_allowed: '该密钥不允许使用此模型',
  key_api_format_disabled: '该密钥已停用此 API 格式',
  api_key_concurrency_limit_reached: '调用方 API Key 并发已达上限',
  auth_api_key_concurrency_limit_reached: '调用方 API Key 并发已达上限',

  // —— 传输与路由策略门（在真正发请求前就被拦下）——
  auth_channel_mismatch: '鉴权通道不匹配',
  auth_snapshot_missing: '缺少该密钥的鉴权快照',
  endpoint_api_format_changed: '端点 API 格式已变更',
  endpoint_inactive: '端点已停用',
  format_conversion_disabled: '该提供商未开启格式转换',
  mapped_model_missing: '缺少映射后的上游模型',
  routing_profile_disallowed_key: '路由策略未允许该密钥',
  routing_profile_disallowed_provider: '路由策略未允许该提供商',
  upstream_url_missing: '缺少上游地址',
  gemini_file_mapping_mismatch: 'Gemini 文件映射不匹配',
  provider_request_body_build_failed: '上游请求体转换失败',
  provider_request_body_missing: '无法构建上游请求体',

  // —— transport_* 系列：该提供商/端点不支持当前这种转发方式 ——
  transport_unsupported: '该传输方式不受支持',
  transport_api_format_mismatch: 'API 格式与端点不匹配',
  transport_api_format_unsupported: '不支持该 API 格式',
  transport_auth_unavailable: '无法获取可用鉴权',
  transport_body_rules_apply_failed: '请求体改写规则执行失败',
  transport_body_rules_unsupported: '不支持请求体改写规则',
  transport_body_rules_unsupported_for_binary_upload: '二进制上传不支持请求体改写规则',
  transport_custom_path_unsupported: '不支持自定义路径',
  transport_endpoint_kind_unsupported: '不支持该端点类型',
  transport_header_rules_apply_failed: '请求头改写规则执行失败',
  transport_header_rules_unsupported: '不支持请求头改写规则',
  transport_oauth_resolution_unsupported: '不支持该 OAuth 解析方式',
  transport_operation_unsupported: '不支持该操作类型',
  transport_profile_unsupported: '不支持该传输配置',
  transport_provider_type_unsupported: '不支持该提供商类型',
  transport_proxy_or_profile_unsupported: '不支持代理或传输配置',
  transport_proxy_unsupported: '不支持该代理',
  transport_snapshot_missing: '缺少传输快照',

  // —— 号池（pool）相关 ——
  pool_group_exhausted: '号池已无可用账号',
  pool_account_blocked: '号池账号已被封禁',
  pool_account_exhausted: '号池账号额度已耗尽',
  pool_active_probe_sealed: '号池探测中，暂不分配',
  pool_cooldown: '号池账号冷却中',
  pool_cost_limit_reached: '号池费用已达上限',
  pool_key_lease_busy: '池内账号正被其他请求占用',
  pool_score_member_missing: '号池评分成员缺失',
}

/** 后端无法归类时使用的占位原因。 */
export const UNCLASSIFIED_CANDIDATE_SKIP_REASON = 'unclassified_candidate_skip_reason'

/**
 * 把候选跳过原因转成中文展示文案。
 *
 * 命中已知标签时返回中文；否则原样返回后端字符串（便于排查新原因），
 * 空值返回空字符串，调用方据此判断是否展示。
 */
export function formatCandidateSkipReason(reason?: string | null): string {
  const normalized = typeof reason === 'string' ? reason.trim() : ''
  if (!normalized) return ''
  return CANDIDATE_SKIP_REASON_LABELS[normalized] ?? normalized
}

/** 是否为"根本没有向上游发起请求"的跳过状态。 */
export function isSkippedCandidateStatus(status?: string | null): boolean {
  return status === 'skipped'
}

/** 是否为"被枚举出来但从未尝试"的候选状态（含跳过与未使用）。 */
export function isNonAttemptedCandidateStatus(status?: string | null): boolean {
  return status === 'skipped' || status === 'available' || status === 'unused'
}
