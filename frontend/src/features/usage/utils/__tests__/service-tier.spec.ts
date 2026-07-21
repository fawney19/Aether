import { describe, expect, it } from 'vitest'

import {
  formatServiceTierFact,
  hasServiceTierFact,
  normalizeServiceTierFact,
  resolveServiceTierFacts,
} from '../service-tier'

describe('service tier facts', () => {
  it('uses the final provider request tier for display and billing', () => {
    const source = {
      service_tier: 'priority',
      actual_service_tier: 'default',
      settlement: {
        settlement_snapshot: {
          pricing_snapshot: {
            requested_processing_tier: 'ignored-requested-snapshot',
            actual_processing_tier: 'ignored-actual-snapshot',
            billing_processing_tier: 'standard',
          },
        },
      },
    }
    const facts = resolveServiceTierFacts(source)

    expect(facts).toEqual({ requested: 'priority' })
    expect(hasServiceTierFact(facts)).toBe(true)
  })

  it('does not infer a tier from the provider response or settlement snapshot', () => {
    const source = {
      actual_service_tier: 'flex',
      settlement: {
        settlement_snapshot: {
          pricing_snapshot: {
            billing_processing_tier: 'priority',
          },
        },
      },
    }

    const facts = resolveServiceTierFacts(source)
    expect(facts).toEqual({ requested: null })
    expect(hasServiceTierFact(facts)).toBe(false)
  })

  it('normalizes only non-empty string facts', () => {
    expect(normalizeServiceTierFact('  Batch  ')).toBe('Batch')
    expect(normalizeServiceTierFact('  ')).toBeNull()
    expect(normalizeServiceTierFact(0)).toBeNull()
  })

  it.each(['priority', 'fast', ' Priority ', 'FAST'])(
    'displays the raw %s tier as Fast',
    (tier) => {
      expect(formatServiceTierFact(tier)).toBe('Fast')
    },
  )

  it('keeps non-fast tier labels unchanged', () => {
    expect(formatServiceTierFact('  Batch  ')).toBe('Batch')
    expect(formatServiceTierFact('  ')).toBeNull()
  })
})
