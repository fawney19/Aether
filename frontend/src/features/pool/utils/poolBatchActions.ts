export type PoolBatchActionValue =
  | 'edit_config'
  | 'export'
  | 'delete'
  | 'refresh_oauth'
  | 'refresh_quota'
  | 'clear_proxy'
  | 'set_proxy'
  | 'update_settings'
  | 'enable'
  | 'disable'

export interface PoolBatchActionOption {
  value: PoolBatchActionValue
  label: string
  hint: string
  destructive?: boolean
}

export const POOL_BATCH_ACTION_OPTIONS: readonly PoolBatchActionOption[] = [
  { value: 'edit_config', label: '编辑配置', hint: '统一修改支持 API、调度参数与自动获取模型设置。' },
  { value: 'refresh_quota', label: '刷新额度', hint: '调用额度刷新接口，适合核对最新配额状态。' },
  { value: 'refresh_oauth', label: '刷新 OAuth', hint: '仅对 OAuth 账号有效，非 OAuth 账号会自动跳过。' },
  { value: 'set_proxy', label: '配置代理', hint: '为选中账号绑定独立代理节点。' },
  { value: 'update_settings', label: '更多设置', hint: '选择性修改 RPM、并发、熔断、备注和代理等配置。' },
  { value: 'clear_proxy', label: '清除代理', hint: '移除账号独立代理，回退到提供商默认代理。' },
  { value: 'enable', label: '启用', hint: '批量启用账号，恢复可调度状态。' },
  { value: 'disable', label: '禁用', hint: '批量禁用账号，保留数据但停止调度。' },
  { value: 'export', label: '导出凭据', hint: '仅导出 OAuth 凭据，其他类型账号将被跳过。' },
  { value: 'delete', label: '删除账号', hint: '永久删除账号数据，执行后不可恢复。', destructive: true },
]
