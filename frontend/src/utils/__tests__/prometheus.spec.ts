import { describe, expect, it } from 'vitest'
import {
  findMetricSamples,
  findMetricValueNumber,
  parsePrometheusSamples,
  sumMetricValues,
} from '../prometheus'

describe('parsePrometheusSamples', () => {
  it('parses labeled samples and finds gate metrics by suffix name', () => {
    const samples = parsePrometheusSamples(`
# HELP aether_gateway_concurrency_in_flight Current number of in-flight operations.
# TYPE aether_gateway_concurrency_in_flight gauge
aether_gateway_concurrency_in_flight{gate="gateway_requests"} 7
aether_gateway_concurrency_rejected_total{gate="gateway_requests"} 12
`)

    expect(
      findMetricValueNumber(samples, 'concurrency_in_flight', {
        gate: 'gateway_requests',
      })
    ).toBe(7)
    expect(
      findMetricValueNumber(samples, 'concurrency_rejected_total', {
        gate: 'gateway_requests',
      })
    ).toBe(12)
  })

  it('sums fallback counters across labeled samples', () => {
    const samples = parsePrometheusSamples(`
decision_remote_total{route_kind="chat",reason="local_decision_miss"} 2
decision_remote_total{route_kind="responses",reason="remote_decision_miss"} 3
`)

    expect(sumMetricValues(samples, 'decision_remote_total')).toBe(5)
  })

  it('finds all matching samples by full or suffix metric name', () => {
    const samples = parsePrometheusSamples(`
aether_gateway_upstream_target_selected_total{target="openai"} 4
aether_gateway_upstream_target_selected_total{target="azure"} 6
aether_gateway_upstream_target_saturated_total{target="openai"} 1
`)

    expect(findMetricSamples(samples, 'upstream_target_selected_total')).toHaveLength(2)
    expect(sumMetricValues(samples, 'upstream_target_selected_total')).toBe(10)
  })
})
