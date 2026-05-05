import { describe, expect, it } from 'vitest'

import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types'
import {
  buildProviderRedactionConfig,
  getProviderRedactionConfig,
  withProviderRedactionConfig,
} from '../providerRedactionPayload'

function makeProvider(overrides: Partial<ProviderWithEndpointsSummary> = {}): ProviderWithEndpointsSummary {
  return {
    id: 'provider-1',
    name: 'Provider One',
    provider_priority: 1,
    keep_priority_on_conversion: false,
    enable_format_conversion: true,
    is_active: true,
    total_endpoints: 0,
    active_endpoints: 0,
    total_keys: 0,
    active_keys: 0,
    total_models: 0,
    active_models: 0,
    global_model_ids: [],
    avg_health_score: 0,
    unhealthy_endpoints: 0,
    api_formats: [],
    endpoint_health_details: [],
    ops_configured: false,
    created_at: '2026-05-02T00:00:00Z',
    updated_at: '2026-05-02T00:00:00Z',
    ...overrides,
  }
}

describe('provider redaction payload helpers', () => {
  it('defaults provider redaction to disabled', () => {
    expect(getProviderRedactionConfig()).toEqual({ enabled: false })
    expect(getProviderRedactionConfig(makeProvider())).toEqual({ enabled: false })
  })

  it('loads existing provider redaction config', () => {
    const provider = makeProvider({ chat_pii_redaction: { enabled: true } })

    expect(getProviderRedactionConfig(provider)).toEqual({ enabled: true })
  })

  it('builds create and update payloads with provider-level enabled only', () => {
    expect(buildProviderRedactionConfig(true)).toEqual({
      chat_pii_redaction: { enabled: true },
    })

    expect(
      withProviderRedactionConfig(
        {
          name: 'Provider One',
          config: { pool_advanced: { global_priority: 10 } },
        },
        false,
      ),
    ).toEqual({
      name: 'Provider One',
      config: {
        pool_advanced: { global_priority: 10 },
        chat_pii_redaction: { enabled: false },
      },
    })
  })

  it('does not include provider-level entity or ttl config', () => {
    const payload = withProviderRedactionConfig({ name: 'Provider One' }, true)

    expect(payload.config.chat_pii_redaction).toEqual({ enabled: true })
    expect(payload.config.chat_pii_redaction).not.toHaveProperty('entities')
    expect(payload.config.chat_pii_redaction).not.toHaveProperty('cache_ttl_seconds')
    expect(payload.config.chat_pii_redaction).not.toHaveProperty('inject_model_instruction')
  })
})
