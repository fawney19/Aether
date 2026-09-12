import type { BodyRuleCondition, BodyRuleConditionOp } from '@/api/endpoints'
import {
  endpointSecretMarkerPayload,
  retainsEndpointSecret,
} from './endpoint-secret-markers'

export type ConditionSource = 'body' | 'original' | 'request_headers'
export type ConditionGroupMode = 'all' | 'any'

export interface EditableConditionLeaf {
  kind: 'leaf'
  path: string
  op: BodyRuleConditionOp
  value: string
  retainValue: boolean
  source: ConditionSource
}

export interface EditableConditionGroup {
  kind: 'group'
  mode: ConditionGroupMode
  children: EditableConditionNode[]
}

export type EditableConditionNode = EditableConditionLeaf | EditableConditionGroup

export const CONDITION_OP_OPTIONS: Array<{ value: BodyRuleConditionOp; label: string }> = [
  { value: 'eq', label: '等于' },
  { value: 'neq', label: '不等于' },
  { value: 'gt', label: '大于' },
  { value: 'lt', label: '小于' },
  { value: 'gte', label: '大于等于' },
  { value: 'lte', label: '小于等于' },
  { value: 'starts_with', label: '开头匹配' },
  { value: 'ends_with', label: '结尾匹配' },
  { value: 'contains', label: '包含' },
  { value: 'matches', label: '正则匹配' },
  { value: 'exists', label: '存在' },
  { value: 'not_exists', label: '不存在' },
  { value: 'in', label: '在列表中' },
  { value: 'type_is', label: '类型是' },
]

const NUMERIC_OPS = new Set(['gt', 'lt', 'gte', 'lte'])
const STRING_OPS = new Set(['starts_with', 'ends_with'])
const TYPE_IS_VALUES = new Set(['string', 'number', 'boolean', 'array', 'object', 'null'])

export function createEmptyConditionLeaf(): EditableConditionLeaf {
  return {
    kind: 'leaf',
    path: '',
    op: 'eq',
    value: '',
    retainValue: false,
    source: 'body',
  }
}

export function createConditionGroup(
  mode: ConditionGroupMode = 'all',
  children: EditableConditionNode[] = [createEmptyConditionLeaf()],
): EditableConditionGroup {
  return {
    kind: 'group',
    mode,
    children,
  }
}

export function cloneEditableCondition(node: EditableConditionNode): EditableConditionNode {
  if (node.kind === 'group') {
    return {
      kind: 'group',
      mode: node.mode,
      children: node.children.map(cloneEditableCondition),
    }
  }
  return { ...node }
}

export function conditionToEditable(condition?: BodyRuleCondition | null): EditableConditionNode | null {
  if (!condition) return null
  if ('all' in condition) {
    return createConditionGroup(
      'all',
      condition.all.map(child => conditionToEditable(child) || createEmptyConditionLeaf()),
    )
  }
  if ('any' in condition) {
    return createConditionGroup(
      'any',
      condition.any.map(child => conditionToEditable(child) || createEmptyConditionLeaf()),
    )
  }
  const source = (condition as { source?: unknown }).source
  return {
    kind: 'leaf',
    path: condition.path || '',
    op: condition.op || 'eq',
    value: condition.value !== undefined
      ? (typeof condition.value === 'string' ? condition.value : JSON.stringify(condition.value))
      : '',
    retainValue: retainsEndpointSecret(condition.value, condition.has_value),
    source: source === 'request_headers' || source === 'headers'
        ? 'request_headers'
        : source === 'original'
          ? 'original'
        : 'body',
  }
}

export function editableConditionToApi(node: EditableConditionNode | null): BodyRuleCondition | undefined {
  if (!node) return undefined

  if (node.kind === 'group') {
    const children = node.children
      .map(child => editableConditionToApi(child))
      .filter((child): child is BodyRuleCondition => !!child)
    if (!children.length) return undefined
    return node.mode === 'all' ? { all: children } : { any: children }
  }

  const path = node.path.trim()
  if (!path) return undefined

  const base = {
    path,
    op: node.op,
    ...(node.source === 'request_headers'
      ? { source: 'request_headers' as const }
      : node.source === 'original'
        ? { source: 'original' as const }
        : {}),
  }

  if (node.op === 'exists' || node.op === 'not_exists') {
    return base
  }

  const raw = node.value.trim()
  const marker = endpointSecretMarkerPayload('has_value', node.retainValue, raw)
  if (!raw) {
    return { ...base, value: '', ...marker }
  }

  try {
    return { ...base, value: JSON.parse(raw), ...marker }
  } catch {
    return { ...base, value: raw, ...marker }
  }
}

export function isConditionValueRequired(op: BodyRuleConditionOp): boolean {
  return op !== 'exists' && op !== 'not_exists'
}

export function getConditionValuePlaceholder(op: BodyRuleConditionOp): string {
  if (op === 'in') return '["a", "b"]'
  if (op === 'type_is') return 'string/number/boolean/...'
  return '值'
}

export function getBodyRuleConditionPathPlaceholder(path: string): string {
  return path.includes('[*]') || /\[\d+-\d+\]/.test(path) ? '$item.字段名' : '字段路径'
}

export function conditionEquals(
  left: EditableConditionNode | null,
  right: EditableConditionNode | null,
): boolean {
  if (left === right) return true
  if (!left || !right) return false
  if (left.kind !== right.kind) return false

  if (left.kind === 'group' && right.kind === 'group') {
    if (left.mode !== right.mode) return false
    if (left.children.length !== right.children.length) return false
    return left.children.every((child, i) => conditionEquals(child, right.children[i]))
  }

  if (left.kind === 'leaf' && right.kind === 'leaf') {
    return left.path === right.path
      && left.op === right.op
      && left.value === right.value
      && left.retainValue === right.retainValue
      && left.source === right.source
  }

  return false
}

export function validateEditableCondition(node: EditableConditionNode | null): string | null {
  if (!node) return null

  if (node.kind === 'group') {
    if (!node.children.length) return '组合条件至少需要一个子条件'
    for (let i = 0; i < node.children.length; i += 1) {
      const err = validateEditableCondition(node.children[i])
      if (err) return `子条件 ${i + 1}: ${err}`
    }
    return null
  }

  const path = node.path.trim()
  if (!path) return '条件路径不能为空'

  if (!isConditionValueRequired(node.op)) return null
  if (node.retainValue) return null

  const raw = node.value.trim()
  let parsed: unknown = raw
  if (raw) {
    try {
      parsed = JSON.parse(raw)
    } catch {
      parsed = raw
    }
  }

  if (NUMERIC_OPS.has(node.op)) {
    if (typeof parsed !== 'number' || Number.isNaN(parsed)) return '数值条件必须填写数字'
    return null
  }

  if (STRING_OPS.has(node.op)) {
    if (typeof parsed !== 'string') return '该条件值必须为字符串'
    return null
  }

  if (node.op === 'matches') {
    if (typeof parsed !== 'string' || !parsed) return '正则条件值不能为空'
    try {
      new RegExp(parsed)
      return null
    } catch (error: unknown) {
      return `正则表达式无效：${error instanceof Error ? error.message : String(error)}`
    }
  }

  if (node.op === 'in') {
    if (!Array.isArray(parsed)) return 'in 条件值必须是 JSON 数组'
    return null
  }

  if (node.op === 'type_is') {
    if (typeof parsed !== 'string' || !TYPE_IS_VALUES.has(parsed)) {
      return 'type_is 仅支持 string/number/boolean/array/object/null'
    }
  }

  return null
}
