// API 格式常量
export const API_FORMATS = {
  // 新模式：endpoint signature key（family:kind，全小写）
  CLAUDE: 'claude:messages',
  CLAUDE_MESSAGES: 'claude:messages',
  OPENAI: 'openai:chat',
  OPENAI_RESPONSES: 'openai:responses',
  OPENAI_RESPONSES_COMPACT: 'openai:responses:compact',
  OPENAI_IMAGE: 'openai:image',
  OPENAI_VIDEO: 'openai:video',
  GEMINI: 'gemini:generate_content',
  GEMINI_GENERATE_CONTENT: 'gemini:generate_content',
  GEMINI_VIDEO: 'gemini:video',
  GEMINI_FILES: 'gemini:files',
} as const

export type APIFormat = typeof API_FORMATS[keyof typeof API_FORMATS]

// API 格式显示名称映射（按品牌分组）
export const API_FORMAT_LABELS: Record<string, string> = {
  [API_FORMATS.CLAUDE_MESSAGES]: 'Claude Messages',
  [API_FORMATS.OPENAI]: 'OpenAI Chat',
  [API_FORMATS.OPENAI_RESPONSES]: 'OpenAI Responses',
  [API_FORMATS.OPENAI_RESPONSES_COMPACT]: 'OpenAI Responses Compact',
  [API_FORMATS.OPENAI_IMAGE]: 'OpenAI Image',
  [API_FORMATS.OPENAI_VIDEO]: 'OpenAI Video',
  [API_FORMATS.GEMINI_GENERATE_CONTENT]: 'Gemini Generate Content',
  [API_FORMATS.GEMINI_VIDEO]: 'Gemini Video',
  [API_FORMATS.GEMINI_FILES]: 'Gemini Files',
  CLAUDE: 'Claude Messages',
  CLAUDE_MESSAGES: 'Claude Messages',
  OPENAI: 'OpenAI Chat',
  OPENAI_RESPONSES: 'OpenAI Responses',
  OPENAI_RESPONSES_COMPACT: 'OpenAI Responses Compact',
  OPENAI_IMAGE: 'OpenAI Image',
  OPENAI_VIDEO: 'OpenAI Video',
  GEMINI: 'Gemini Generate Content',
  GEMINI_GENERATE_CONTENT: 'Gemini Generate Content',
  GEMINI_VIDEO: 'Gemini Video',
  GEMINI_FILES: 'Gemini Files',
}

// API 格式缩写映射（用于空间紧凑的显示场景）
export const API_FORMAT_SHORT: Record<string, string> = {
  [API_FORMATS.OPENAI]: 'O',
  [API_FORMATS.OPENAI_RESPONSES]: 'OR',
  [API_FORMATS.OPENAI_RESPONSES_COMPACT]: 'ORC',
  [API_FORMATS.OPENAI_IMAGE]: 'OI',
  [API_FORMATS.OPENAI_VIDEO]: 'OV',
  [API_FORMATS.CLAUDE_MESSAGES]: 'CM',
  [API_FORMATS.GEMINI_GENERATE_CONTENT]: 'G',
  [API_FORMATS.GEMINI_VIDEO]: 'GV',
  [API_FORMATS.GEMINI_FILES]: 'GF',
  OPENAI: 'O',
  OPENAI_RESPONSES: 'OR',
  OPENAI_RESPONSES_COMPACT: 'ORC',
  OPENAI_IMAGE: 'OI',
  OPENAI_VIDEO: 'OV',
  CLAUDE: 'CM',
  CLAUDE_MESSAGES: 'CM',
  GEMINI: 'G',
  GEMINI_GENERATE_CONTENT: 'G',
  GEMINI_VIDEO: 'GV',
  GEMINI_FILES: 'GF',
}

// API 格式排序顺序（统一的显示顺序）
export const API_FORMAT_ORDER: string[] = [
  API_FORMATS.OPENAI,
  API_FORMATS.OPENAI_RESPONSES,
  API_FORMATS.OPENAI_RESPONSES_COMPACT,
  API_FORMATS.OPENAI_IMAGE,
  API_FORMATS.OPENAI_VIDEO,
  API_FORMATS.CLAUDE_MESSAGES,
  API_FORMATS.GEMINI_GENERATE_CONTENT,
  API_FORMATS.GEMINI_VIDEO,
  API_FORMATS.GEMINI_FILES,
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
  responses: 'Responses',
  'responses:compact': 'Responses Compact',
  messages: 'Messages',
  generate_content: 'Generate Content',
  image: 'Image',
  video: 'Video',
  files: 'Files',
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
  // Only normalize current enum-style frontend constants. Retired API format ids
  // are migrated in the database and intentionally do not map at runtime.
  switch (raw.toUpperCase()) {
    case 'CLAUDE':
    case 'CLAUDE_MESSAGES':
      return API_FORMATS.CLAUDE_MESSAGES
    case 'OPENAI':
      return API_FORMATS.OPENAI
    case 'OPENAI_RESPONSES':
      return API_FORMATS.OPENAI_RESPONSES
    case 'OPENAI_RESPONSES_COMPACT':
      return API_FORMATS.OPENAI_RESPONSES_COMPACT
    case 'OPENAI_IMAGE':
      return API_FORMATS.OPENAI_IMAGE
    case 'OPENAI_VIDEO':
      return API_FORMATS.OPENAI_VIDEO
    case 'GEMINI':
    case 'GEMINI_GENERATE_CONTENT':
      return API_FORMATS.GEMINI_GENERATE_CONTENT
    case 'GEMINI_VIDEO':
      return API_FORMATS.GEMINI_VIDEO
    case 'GEMINI_FILES':
      return API_FORMATS.GEMINI_FILES
    default:
      return raw.toLowerCase()
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
    || API_FORMAT_SHORT[upper]
    || normalized.substring(0, 2)
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

// openai family 格式只支持 bearer（Authorization header），不允许覆盖认证方式
export function formatSupportsAuthOverride(format: string): boolean {
  return parseApiFormat(normalizeApiFormatAlias(format)).family !== 'openai'
}
