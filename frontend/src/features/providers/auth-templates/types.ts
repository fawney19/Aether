/**
 * 提供商认证模板类型定义
 */

/**
 * 表单字段类型
 */
export type FieldType = 'text' | 'password' | 'select' | 'textarea'

/**
 * 表单字段定义
 */
export interface AuthTemplateField {
  /** 字段 key（用于表单数据） */
  key: string
  /** 显示标签 */
  label: string
  /** 字段类型 */
  type: FieldType
  /** 占位符 */
  placeholder?: string
  /** 帮助文本 */
  helpText?: string
  /** 是否必填 */
  required?: boolean
  /** 是否为敏感字段（使用 masked 输入） */
  sensitive?: boolean
  /** select 类型的选项 */
  options?: Array<{ value: string; label: string }>
  /** 默认值 */
  defaultValue?: string
  /** inline 布局时的 flex 比例（默认 1） */
  flex?: number
}

/**
 * 表单字段分组
 */
export interface AuthTemplateFieldGroup {
  /** 分组标题（可选，为空则不显示标题） */
  title?: string
  /** 分组内的字段 */
  fields: AuthTemplateField[]
  /** 是否可折叠（默认展开） */
  collapsible?: boolean
  /** 折叠时的默认状态：true=展开，false=收起 */
  defaultExpanded?: boolean
  /** 是否有启用开关（与 collapsible 配合使用） */
  hasToggle?: boolean
  /** 启用开关对应的表单字段 key */
  toggleKey?: string
  /** 布局方式：'vertical'(默认) 或 'inline'(同行显示) */
  layout?: 'vertical' | 'inline'
}

/**
 * 余额附加信息项
 */
export interface BalanceExtraItem {
  /** 显示标签 */
  label: string
  /** 显示值 */
  value: string
  /** 百分比数值 (0-100)，用于进度条显示 */
  percent?: number
  /** 重置时间戳（Unix 秒），用于倒计时显示 */
  resetsAt?: number
  /** 可选的提示文本 */
  tooltip?: string
}

// ==================== 通用字段定义 ====================

/**
 * 代理节点 ID 字段
 */
export const PROXY_NODE_FIELD: AuthTemplateField = {
  key: 'proxy_node_id',
  label: '代理节点',
  type: 'select',
  placeholder: '选择代理节点...',
  required: false,
}

/**
 * 通用代理配置字段组（可折叠，带启用开关）
 * 由 ProviderAuthDialog 特殊渲染为代理节点选择器
 */
export const PROXY_FIELD_GROUP: AuthTemplateFieldGroup = {
  title: '代理配置',
  fields: [PROXY_NODE_FIELD],
  collapsible: true,
  defaultExpanded: false,
  hasToggle: true,
  toggleKey: 'proxy_enabled',
}

/**
 * 构建代理配置（仅代理节点模式）
 *
 * @param formData 表单数据
 * @returns 代理配置对象，展开到 connector.config 中
 */
export function buildProxyConfig(formData: Record<string, unknown>): { proxy_node_id?: string } {
  if (!formData.proxy_enabled || !formData.proxy_node_id) {
    return {}
  }
  return typeof formData.proxy_node_id === 'string'
    ? { proxy_node_id: formData.proxy_node_id }
    : {}
}

/**
 * 解析代理配置
 *
 * @param config connector.config 对象
 * @returns 表单数据
 */
export function parseProxyConfig(config: Record<string, unknown> | null | undefined): Record<string, unknown> {
  // 代理节点模式
  if (config?.proxy_node_id) {
    return {
      proxy_enabled: true,
      proxy_node_id: config.proxy_node_id,
    }
  }

  // 兼容旧数据（手动 URL 模式）- 标记为启用但无节点
  if (config?.proxy) {
    return {
      proxy_enabled: true,
      proxy_node_id: '',
    }
  }

  return {
    proxy_enabled: false,
    proxy_node_id: '',
  }
}
