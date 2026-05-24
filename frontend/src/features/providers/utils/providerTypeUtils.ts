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
  'kiro',
  'grok',
  'windsurf',
])

export const isOAuthAccountProviderType = (providerType?: string | null): boolean =>
  oauthAccountProviderTypes.has((providerType || '').toLowerCase())

export const isKeyManagedProviderType = (providerType?: string | null): boolean =>
  !isOAuthAccountProviderType(providerType)
