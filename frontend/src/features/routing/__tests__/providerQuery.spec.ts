import { describe, expect, it } from 'vitest'

import { DEFAULT_ROUTING_POLICY_MODEL } from '../utils/routingPolicy'
import { buildRoutingProviderSummaryQuery } from '../utils/providerQuery'

describe('routing provider query', () => {
  it('filters provider sorting by the selected GlobalModel ID', () => {
    expect(buildRoutingProviderSummaryQuery(
      'claude-fable-5-1',
      'global-claude-fable-5-1',
      'provider',
    )).toEqual({
      page: 1,
      page_size: 9999,
      model_id: 'global-claude-fable-5-1',
    })
  })

  it('keeps unified sorting and key sorting unfiltered', () => {
    const expected = { page: 1, page_size: 9999 }

    expect(buildRoutingProviderSummaryQuery(DEFAULT_ROUTING_POLICY_MODEL, undefined, 'provider'))
      .toEqual(expected)
    expect(buildRoutingProviderSummaryQuery('claude-fable-5-1', 'global-claude-fable-5-1', 'global_key'))
      .toEqual(expected)
  })

  it('does not fall back to all providers before a model ID is available', () => {
    expect(buildRoutingProviderSummaryQuery('claude-fable-5-1', undefined, 'provider')).toBeNull()
  })
})
