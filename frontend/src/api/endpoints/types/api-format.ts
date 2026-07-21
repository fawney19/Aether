// API 格式常量
export const API_FORMATS = {
  // 新模式：endpoint signature key（family:kind，全小写）
  CLAUDE: 'claude:messages',
  CLAUDE_MESSAGES: 'claude:messages',
  OPENAI: 'openai:chat',
  OPENAI_RESPONSES: 'openai:responses',
  OPENAI_RESPONSES_COMPACT: 'openai:responses:compact',
  OPENAI_SEARCH: 'openai:search',
  OPENAI_IMAGE: 'openai:image',
  OPENAI_VIDEO: 'openai:video',
  OPENAI_EMBEDDING: 'openai:embedding',
  OPENAI_RERANK: 'openai:rerank',
  GEMINI: 'gemini:generate_content',
  GEMINI_GENERATE_CONTENT: 'gemini:generate_content',
  GEMINI_INTERACTIONS: 'gemini:interactions',
  GEMINI_VIDEO: 'gemini:video',
  GEMINI_FILES: 'gemini:files',
  GEMINI_EMBEDDING: 'gemini:embedding',
  JINA_EMBEDDING: 'jina:embedding',
  JINA_RERANK: 'jina:rerank',
  DOUBAO_EMBEDDING: 'doubao:embedding',
  ALIYUN_MULTIMODAL_EMBEDDING: 'aliyun:multimodal_embedding',
} as const

export type APIFormat = typeof API_FORMATS[keyof typeof API_FORMATS]

// API 格式显示名称映射（按品牌分组）
export const API_FORMAT_LABELS: Record<string, string> = {
  [API_FORMATS.CLAUDE_MESSAGES]: 'Claude Messages',
  [API_FORMATS.OPENAI]: 'OpenAI Chat',
  [API_FORMATS.OPENAI_RESPONSES]: 'OpenAI Responses',
  [API_FORMATS.OPENAI_RESPONSES_COMPACT]: 'OpenAI Responses Compact',
  [API_FORMATS.OPENAI_SEARCH]: 'OpenAI Search',
  [API_FORMATS.OPENAI_IMAGE]: 'OpenAI Image',
  [API_FORMATS.OPENAI_VIDEO]: 'OpenAI Video',
  [API_FORMATS.OPENAI_EMBEDDING]: 'OpenAI Embedding',
  [API_FORMATS.OPENAI_RERANK]: 'OpenAI Rerank',
  [API_FORMATS.GEMINI_GENERATE_CONTENT]: 'Gemini Generate Content',
  [API_FORMATS.GEMINI_INTERACTIONS]: 'Gemini Interactions',
  [API_FORMATS.GEMINI_VIDEO]: 'Gemini Video',
  [API_FORMATS.GEMINI_FILES]: 'Gemini Files',
  [API_FORMATS.GEMINI_EMBEDDING]: 'Gemini Embedding',
  [API_FORMATS.JINA_EMBEDDING]: 'Jina Embedding',
  [API_FORMATS.JINA_RERANK]: 'Jina Rerank',
  [API_FORMATS.DOUBAO_EMBEDDING]: 'Doubao Embedding',
  [API_FORMATS.ALIYUN_MULTIMODAL_EMBEDDING]: 'Aliyun Multimodal Embedding',
  CLAUDE: 'Claude Messages',
  CLAUDE_MESSAGES: 'Claude Messages',
  OPENAI: 'OpenAI Chat',
  OPENAI_RESPONSES: 'OpenAI Responses',
  OPENAI_RESPONSES_COMPACT: 'OpenAI Responses Compact',
  OPENAI_SEARCH: 'OpenAI Search',
  OPENAI_IMAGE: 'OpenAI Image',
  OPENAI_VIDEO: 'OpenAI Video',
  OPENAI_EMBEDDING: 'OpenAI Embedding',
  OPENAI_RERANK: 'OpenAI Rerank',
  GEMINI: 'Gemini Generate Content',
  GEMINI_GENERATE_CONTENT: 'Gemini Generate Content',
  GEMINI_INTERACTIONS: 'Gemini Interactions',
  GEMINI_VIDEO: 'Gemini Video',
  GEMINI_FILES: 'Gemini Files',
  GEMINI_EMBEDDING: 'Gemini Embedding',
  JINA_EMBEDDING: 'Jina Embedding',
  JINA_RERANK: 'Jina Rerank',
  DOUBAO_EMBEDDING: 'Doubao Embedding',
  ALIYUN_MULTIMODAL_EMBEDDING: 'Aliyun Multimodal Embedding',
}

// API 格式缩写映射（用于空间紧凑的显示场景）
export const API_FORMAT_SHORT: Record<string, string> = {
  [API_FORMATS.OPENAI]: 'O',
  [API_FORMATS.OPENAI_RESPONSES]: 'OR',
  [API_FORMATS.OPENAI_RESPONSES_COMPACT]: 'ORC',
  [API_FORMATS.OPENAI_SEARCH]: 'OS',
  [API_FORMATS.OPENAI_IMAGE]: 'OI',
  [API_FORMATS.OPENAI_VIDEO]: 'OV',
  [API_FORMATS.OPENAI_EMBEDDING]: 'OE',
  [API_FORMATS.OPENAI_RERANK]: 'ORR',
  [API_FORMATS.CLAUDE_MESSAGES]: 'CM',
  [API_FORMATS.GEMINI_GENERATE_CONTENT]: 'G',
  [API_FORMATS.GEMINI_INTERACTIONS]: 'GI',
  [API_FORMATS.GEMINI_VIDEO]: 'GV',
  [API_FORMATS.GEMINI_FILES]: 'GF',
  [API_FORMATS.GEMINI_EMBEDDING]: 'GE',
  [API_FORMATS.JINA_EMBEDDING]: 'JE',
  [API_FORMATS.JINA_RERANK]: 'JR',
  [API_FORMATS.DOUBAO_EMBEDDING]: 'DE',
  [API_FORMATS.ALIYUN_MULTIMODAL_EMBEDDING]: 'AE',
  OPENAI: 'O',
  OPENAI_RESPONSES: 'OR',
  OPENAI_RESPONSES_COMPACT: 'ORC',
  OPENAI_SEARCH: 'OS',
  OPENAI_IMAGE: 'OI',
  OPENAI_VIDEO: 'OV',
  OPENAI_EMBEDDING: 'OE',
  OPENAI_RERANK: 'ORR',
  CLAUDE: 'CM',
  CLAUDE_MESSAGES: 'CM',
  GEMINI: 'G',
  GEMINI_GENERATE_CONTENT: 'G',
  GEMINI_INTERACTIONS: 'GI',
  GEMINI_VIDEO: 'GV',
  GEMINI_FILES: 'GF',
  GEMINI_EMBEDDING: 'GE',
  JINA_EMBEDDING: 'JE',
  JINA_RERANK: 'JR',
  DOUBAO_EMBEDDING: 'DE',
  ALIYUN_MULTIMODAL_EMBEDDING: 'AE',
}

// API 格式排序顺序（统一的显示顺序）
export const API_FORMAT_ORDER: string[] = [
  API_FORMATS.OPENAI,
  API_FORMATS.OPENAI_RESPONSES,
  API_FORMATS.OPENAI_RESPONSES_COMPACT,
  API_FORMATS.OPENAI_SEARCH,
  API_FORMATS.OPENAI_EMBEDDING,
  API_FORMATS.OPENAI_RERANK,
  API_FORMATS.OPENAI_IMAGE,
  API_FORMATS.OPENAI_VIDEO,
  API_FORMATS.CLAUDE_MESSAGES,
  API_FORMATS.GEMINI_GENERATE_CONTENT,
  API_FORMATS.GEMINI_INTERACTIONS,
  API_FORMATS.GEMINI_EMBEDDING,
  API_FORMATS.GEMINI_VIDEO,
  API_FORMATS.GEMINI_FILES,
  API_FORMATS.JINA_EMBEDDING,
  API_FORMATS.JINA_RERANK,
  API_FORMATS.DOUBAO_EMBEDDING,
  API_FORMATS.ALIYUN_MULTIMODAL_EMBEDDING,
]

// Family 显示名称映射
export const API_FORMAT_FAMILY_LABELS: Record<string, string> = {
  openai: 'OpenAI',
  claude: 'Claude',
  gemini: 'Gemini',
  jina: 'Jina',
  doubao: 'Doubao',
  aliyun: 'Aliyun',
}

// Kind 显示名称映射
export const API_FORMAT_KIND_LABELS: Record<string, string> = {
  chat: 'Chat',
  responses: 'Responses',
  'responses:compact': 'Responses Compact',
  search: 'Search',
  messages: 'Messages',
  generate_content: 'Generate Content',
  interactions: 'Interactions',
  image: 'Image',
  video: 'Video',
  files: 'Files',
  embedding: 'Embedding',
  rerank: 'Rerank',
}

// Family 排序顺序
const FAMILY_ORDER = ['openai', 'claude', 'gemini', 'jina', 'doubao', 'aliyun']

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
    case 'OPENAI_SEARCH':
    case 'SEARCH':
      return API_FORMATS.OPENAI_SEARCH
    case 'OPENAI_IMAGE':
      return API_FORMATS.OPENAI_IMAGE
    case 'OPENAI_VIDEO':
      return API_FORMATS.OPENAI_VIDEO
    case 'OPENAI_EMBEDDING':
      return API_FORMATS.OPENAI_EMBEDDING
    case 'OPENAI_RERANK':
      return API_FORMATS.OPENAI_RERANK
    case 'GEMINI':
    case 'GEMINI_GENERATE_CONTENT':
      return API_FORMATS.GEMINI_GENERATE_CONTENT
    case 'GEMINI_INTERACTIONS':
      return API_FORMATS.GEMINI_INTERACTIONS
    case 'GEMINI_VIDEO':
      return API_FORMATS.GEMINI_VIDEO
    case 'GEMINI_FILES':
      return API_FORMATS.GEMINI_FILES
    case 'GEMINI_EMBEDDING':
      return API_FORMATS.GEMINI_EMBEDDING
    case 'JINA_EMBEDDING':
      return API_FORMATS.JINA_EMBEDDING
    case 'JINA_RERANK':
      return API_FORMATS.JINA_RERANK
    case 'DOUBAO_EMBEDDING':
      return API_FORMATS.DOUBAO_EMBEDDING
    case 'ALIYUN_MULTIMODAL_EMBEDDING':
    case 'ALIYUN_EMBEDDING':
    case 'DASHSCOPE_MULTIMODAL_EMBEDDING':
    case 'DASHSCOPE_EMBEDDING':
      return API_FORMATS.ALIYUN_MULTIMODAL_EMBEDDING
    default:
      switch (raw.toLowerCase()) {
        case 'dashscope:multimodal_embedding':
        case 'aliyun_multimodal_embedding':
        case 'dashscope_multimodal_embedding':
          return API_FORMATS.ALIYUN_MULTIMODAL_EMBEDDING
        default:
          return raw.toLowerCase()
      }
  }
}

export function apiFormatPermissionCovers(
  allowedFormat: string | null | undefined,
  requestedFormat: string | null | undefined,
): boolean {
  const allowed = normalizeApiFormatAlias(allowedFormat)
  const requested = normalizeApiFormatAlias(requestedFormat)
  return Boolean(allowed)
    && Boolean(requested)
    && (allowed === requested
      || (allowed === API_FORMATS.OPENAI_RESPONSES
        && (requested === API_FORMATS.OPENAI_RESPONSES_COMPACT
          || requested === API_FORMATS.OPENAI_SEARCH)))
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
  return [...formats].sort(compareApiFormats)
}

export function compareApiFormats(a: string, b: string): number {
  const aIdx = API_FORMAT_ORDER.indexOf(normalizeApiFormatAlias(a))
  const bIdx = API_FORMAT_ORDER.indexOf(normalizeApiFormatAlias(b))
  if (aIdx === -1 && bIdx === -1) return 0
  if (aIdx === -1) return 1
  if (bIdx === -1) return -1
  return aIdx - bIdx
}

// openai family 格式只支持 bearer（Authorization header），不允许覆盖认证方式
export function formatSupportsAuthOverride(format: string): boolean {
  return parseApiFormat(normalizeApiFormatAlias(format)).family !== 'openai'
}
