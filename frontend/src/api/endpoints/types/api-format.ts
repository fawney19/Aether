// API 格式常量
export const API_FORMATS = {
  // 新模式：endpoint signature key（family:kind，全小写）
  CLAUDE: 'claude:chat',
  CLAUDE_CLI: 'claude:cli',
  OPENAI: 'openai:chat',
  OPENAI_RESPONSES: 'openai:responses',
  OPENAI_RESPONSES_COMPACT: 'openai:responses:compact',
  OPENAI_IMAGE: 'openai:image',
  OPENAI_VIDEO: 'openai:video',
  GEMINI: 'gemini:chat',
  GEMINI_CLI: 'gemini:cli',
  GEMINI_VIDEO: 'gemini:video',
} as const

export type APIFormat = typeof API_FORMATS[keyof typeof API_FORMATS]

// API 格式显示名称映射（按品牌分组：Chat 在前，CLI/Video 在后）
export const API_FORMAT_LABELS: Record<string, string> = {
  [API_FORMATS.CLAUDE]: 'Claude Chat',
  [API_FORMATS.CLAUDE_CLI]: 'Claude CLI',
  [API_FORMATS.OPENAI]: 'OpenAI Chat',
  [API_FORMATS.OPENAI_RESPONSES]: 'OpenAI Responses',
  [API_FORMATS.OPENAI_RESPONSES_COMPACT]: 'OpenAI Responses Compact',
  [API_FORMATS.OPENAI_IMAGE]: 'OpenAI Image',
  [API_FORMATS.OPENAI_VIDEO]: 'OpenAI Video',
  [API_FORMATS.GEMINI]: 'Gemini Chat',
  [API_FORMATS.GEMINI_CLI]: 'Gemini CLI',
  [API_FORMATS.GEMINI_VIDEO]: 'Gemini Video',
  // legacy 兼容（仅用于展示历史数据）
  CLAUDE: 'Claude Chat',
  CLAUDE_CLI: 'Claude CLI',
  OPENAI: 'OpenAI Chat',
  LEGACY_OPENAI_CLI: 'OpenAI Responses',
  LEGACY_OPENAI_COMPACT: 'OpenAI Responses Compact',
  OPENAI_RESPONSES: 'OpenAI Responses',
  OPENAI_RESPONSES_COMPACT: 'OpenAI Responses Compact',
  OPENAI_IMAGE: 'OpenAI Image',
  OPENAI_VIDEO: 'OpenAI Video',
  GEMINI: 'Gemini Chat',
  GEMINI_CLI: 'Gemini CLI',
  GEMINI_VIDEO: 'Gemini Video',
}

// API 格式缩写映射（用于空间紧凑的显示场景）
export const API_FORMAT_SHORT: Record<string, string> = {
  [API_FORMATS.OPENAI]: 'O',
  [API_FORMATS.OPENAI_RESPONSES]: 'OR',
  [API_FORMATS.OPENAI_RESPONSES_COMPACT]: 'ORC',
  [API_FORMATS.OPENAI_IMAGE]: 'OI',
  [API_FORMATS.OPENAI_VIDEO]: 'OV',
  [API_FORMATS.CLAUDE]: 'C',
  [API_FORMATS.CLAUDE_CLI]: 'CC',
  [API_FORMATS.GEMINI]: 'G',
  [API_FORMATS.GEMINI_CLI]: 'GC',
  [API_FORMATS.GEMINI_VIDEO]: 'GV',
  // legacy 兼容（仅用于展示历史数据）
  OPENAI: 'O',
  LEGACY_OPENAI_CLI: 'OR',
  LEGACY_OPENAI_COMPACT: 'ORC',
  OPENAI_RESPONSES: 'OR',
  OPENAI_RESPONSES_COMPACT: 'ORC',
  OPENAI_IMAGE: 'OI',
  OPENAI_VIDEO: 'OV',
  CLAUDE: 'C',
  CLAUDE_CLI: 'CC',
  GEMINI: 'G',
  GEMINI_CLI: 'GC',
  GEMINI_VIDEO: 'GV',
}

// API 格式排序顺序（统一的显示顺序）
export const API_FORMAT_ORDER: string[] = [
  API_FORMATS.OPENAI,
  API_FORMATS.OPENAI_RESPONSES,
  API_FORMATS.OPENAI_RESPONSES_COMPACT,
  API_FORMATS.OPENAI_IMAGE,
  API_FORMATS.OPENAI_VIDEO,
  API_FORMATS.CLAUDE,
  API_FORMATS.CLAUDE_CLI,
  API_FORMATS.GEMINI,
  API_FORMATS.GEMINI_CLI,
  API_FORMATS.GEMINI_VIDEO,
]

// Family 显示名称映射
export const API_FORMAT_FAMILY_LABELS: Record<string, string> = {
  openai: 'OpenAI',
  claude: 'Claude',
  gemini: 'Gemini',
}

// Kind 显示名称映射
export const API_FORMAT_KIND_LABELS: Record<string, string> = {
  chat: 'Chat',
  cli: 'CLI',
  responses: 'Responses',
  'responses:compact': 'Responses Compact',
  compact: 'Compact',
  image: 'Image',
  video: 'Video',
}

// Family 排序顺序
const FAMILY_ORDER = ['openai', 'claude', 'gemini']

// 工具函数：从 API 格式中提取 family 和 kind
export function parseApiFormat(format: string): { family: string; kind: string } {
  const idx = format.indexOf(':')
  if (idx === -1) return { family: format.toLowerCase(), kind: '' }
  return { family: format.slice(0, idx).toLowerCase(), kind: format.slice(idx + 1).toLowerCase() }
}

export function normalizeApiFormatAlias(format: string | null | undefined): string {
  const raw = format?.trim() ?? ''
  switch (raw.toLowerCase()) {
    case 'openai:cli':
      return API_FORMATS.OPENAI_RESPONSES
    case 'openai:compact':
      return API_FORMATS.OPENAI_RESPONSES_COMPACT
    default:
      break
  }

  switch (raw.toUpperCase()) {
    case 'OPENAI_CLI':
      return API_FORMATS.OPENAI_RESPONSES
    case 'OPENAI_COMPACT':
      return API_FORMATS.OPENAI_RESPONSES_COMPACT
    default:
      return raw
  }
}

// 工具函数：按 family 分组并排序 API 格式数组
export interface ApiFormatGroup {
  family: string
  label: string
  formats: string[]
}

export function groupApiFormats(formats: string[]): ApiFormatGroup[] {
  const sorted = sortApiFormats(formats)
  const groups = new Map<string, string[]>()
  for (const f of sorted) {
    const { family } = parseApiFormat(normalizeApiFormatAlias(f))
    if (!groups.has(family)) groups.set(family, [])
    groups.get(family)?.push(f)
  }
  return [...groups.entries()]
    .sort(([a], [b]) => {
      const ai = FAMILY_ORDER.indexOf(a)
      const bi = FAMILY_ORDER.indexOf(b)
      if (ai === -1 && bi === -1) return 0
      if (ai === -1) return 1
      if (bi === -1) return -1
      return ai - bi
    })
    .map(([family, fmts]) => ({
      family,
      label: API_FORMAT_FAMILY_LABELS[family] || family,
      formats: fmts,
    }))
}

// 工具函数：将 API 格式签名转为友好显示名称
export function formatApiFormat(format: string | null | undefined): string {
  if (!format) return '-'
  const normalized = normalizeApiFormatAlias(format)
  if (!normalized) return '-'
  const upper = normalized.toUpperCase()
  return API_FORMAT_LABELS[normalized]
    || API_FORMAT_LABELS[normalized.toLowerCase()]
    || API_FORMAT_LABELS[legacyUppercaseApiFormatKey(upper)]
    || API_FORMAT_LABELS[upper]
    || normalized
}

export function formatApiFormatShort(format: string | null | undefined): string {
  if (!format) return '-'
  const normalized = normalizeApiFormatAlias(format)
  if (!normalized) return '-'
  const upper = normalized.toUpperCase()
  return API_FORMAT_SHORT[normalized]
    || API_FORMAT_SHORT[normalized.toLowerCase()]
    || API_FORMAT_SHORT[legacyUppercaseApiFormatKey(upper)]
    || API_FORMAT_SHORT[upper]
    || normalized.substring(0, 2)
}

function legacyUppercaseApiFormatKey(value: string): string {
  switch (value) {
    case 'OPENAI_CLI':
      return 'LEGACY_OPENAI_CLI'
    case 'OPENAI_COMPACT':
      return 'LEGACY_OPENAI_COMPACT'
    default:
      return value
  }
}

// 工具函数：按标准顺序排序 API 格式数组
export function sortApiFormats(formats: string[]): string[] {
  return [...formats].sort((a, b) => {
    const aIdx = API_FORMAT_ORDER.indexOf(normalizeApiFormatAlias(a))
    const bIdx = API_FORMAT_ORDER.indexOf(normalizeApiFormatAlias(b))
    if (aIdx === -1 && bIdx === -1) return 0
    if (aIdx === -1) return 1
    if (bIdx === -1) return -1
    return aIdx - bIdx
  })
}
