/**
 * Provider 类型判断工具函数。
 *
 * 区分"密钥型"和"OAuth 账号型"两类 Provider，影响前端显示标签和操作入口。
 */

const oauthAccountProviderTypes = new Set([
  'claude_code',
  'codex',
  'chatgpt_web',
  'gemini_cli',
  'antigravity',
  'antigravity_cli',
  'kiro',
  'grok',
  'windsurf',
])

export const PROVIDER_TYPE_DISPLAY_NAMES: Record<string, string> = {
  custom: '自定义',
  vertex_ai: 'Vertex AI',
  claude_code: 'ClaudeCode',
  codex: 'Codex',
  chatgpt_web: 'ChatGPT Web',
  gemini_cli: 'Gemini CLI',
  antigravity: 'Antigravity',
  antigravity_cli: 'Antigravity CLI',
  kiro: 'Kiro',
  grok: 'Grok',
  windsurf: 'Windsurf',
}

export const isOAuthAccountProviderType = (providerType?: string | null): boolean =>
  oauthAccountProviderTypes.has((providerType || '').toLowerCase())

export const isKeyManagedProviderType = (providerType?: string | null): boolean =>
  !isOAuthAccountProviderType(providerType)

export const isAntigravityRuntimeProviderType = (providerType?: string | null): boolean => {
  const normalized = (providerType || '').trim().toLowerCase()
  return normalized === 'antigravity' || normalized === 'antigravity_cli'
}

export const getProviderTypeDisplayName = (providerType?: string | null): string => {
  const normalized = (providerType || '').trim().toLowerCase()
  return normalized ? (PROVIDER_TYPE_DISPLAY_NAMES[normalized] || providerType || normalized) : ''
}
