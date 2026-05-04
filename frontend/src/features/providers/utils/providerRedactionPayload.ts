import type { ProviderConfig, ProviderWithEndpointsSummary } from '@/api/endpoints/types'
import { normalizeChatPiiRedactionProviderConfig } from '@/api/endpoints/types'

export const DEFAULT_PROVIDER_REDACTION_CONFIG = Object.freeze({ enabled: false })

export function getProviderRedactionConfig(provider?: ProviderWithEndpointsSummary | null) {
  return normalizeChatPiiRedactionProviderConfig(provider?.chat_pii_redaction)
}

export function buildProviderRedactionConfig(enabled: boolean): ProviderConfig {
  return {
    chat_pii_redaction: { enabled },
  }
}

export function withProviderRedactionConfig<T extends { config?: ProviderConfig | null }>(
  payload: Omit<T, 'config'> & { config?: ProviderConfig | null },
  enabled: boolean,
): T {
  return {
    ...payload,
    config: {
      ...(payload.config ?? {}),
      ...buildProviderRedactionConfig(enabled),
    },
  } as T
}
