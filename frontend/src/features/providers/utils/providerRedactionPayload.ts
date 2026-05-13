import type { ProviderConfig, ProviderWithEndpointsSummary } from '@/api/endpoints/types'
import { normalizeChatPiiRedactionProviderConfig } from '@/api/endpoints/types'

export const DEFAULT_PROVIDER_REDACTION_CONFIG = Object.freeze({ enabled: false })

type ProviderConfigWithRedaction = ProviderConfig & Required<Pick<ProviderConfig, 'chat_pii_redaction'>>

type ProviderRedactionPayload<TPayload extends object> = Omit<TPayload, 'config'> & {
  config: ProviderConfigWithRedaction
}

export function getProviderRedactionConfig(provider?: ProviderWithEndpointsSummary | null) {
  return normalizeChatPiiRedactionProviderConfig(provider?.chat_pii_redaction)
}

export function buildProviderRedactionConfig(enabled: boolean): ProviderConfigWithRedaction {
  return {
    chat_pii_redaction: { enabled },
  }
}

export function withProviderRedactionConfig<TPayload extends object>(
  payload: TPayload & { config?: ProviderConfig | null },
  enabled: boolean,
): ProviderRedactionPayload<TPayload> {
  const { config, ...payloadWithoutConfig } = payload

  return {
    ...payloadWithoutConfig,
    config: {
      ...(config ?? {}),
      ...buildProviderRedactionConfig(enabled),
    },
  }
}
