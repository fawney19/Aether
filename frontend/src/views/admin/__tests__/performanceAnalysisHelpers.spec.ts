import { describe, expect, it } from 'vitest'
import {
  buildProviderPerformanceChartData,
  formatDurationMs,
  formatProviderPerformanceMetric,
} from '../performanceAnalysisHelpers'
import type { ProviderPerformanceItem, ProviderPerformanceTimelineItem } from '@/api/admin'

describe('performanceAnalysisHelpers', () => {
  it('formats null provider metrics as placeholders', () => {
    expect(formatProviderPerformanceMetric(null, 'ms')).toBe('-')
    expect(formatProviderPerformanceMetric(undefined, '/s')).toBe('-')
    expect(formatProviderPerformanceMetric(Number.NaN)).toBe('-')
    expect(formatProviderPerformanceMetric(18.456, '/s')).toBe('18.46/s')
  })

  it('formats millisecond metrics as seconds above one second', () => {
    expect(formatDurationMs(999, 0)).toBe('999ms')
    expect(formatDurationMs(1000, 0)).toBe('1.00s')
    expect(formatDurationMs(80148, 0)).toBe('80.15s')
    expect(formatProviderPerformanceMetric(80148, 'ms', 0)).toBe('80.15s')
    expect(formatProviderPerformanceMetric(99.456, 'ms')).toBe('99.46ms')
  })

  it('builds stable provider trend datasets with null gaps', () => {
    const providers: Pick<ProviderPerformanceItem, 'provider_id' | 'provider'>[] = [
      { provider_id: 'provider-a', provider: 'OpenAI' },
      { provider_id: 'provider-b', provider: 'Anthropic' },
    ]
    const timeline: ProviderPerformanceTimelineItem[] = [
      {
        date: '2024-03-21',
        provider_id: 'provider-a',
        provider: 'OpenAI',
        request_count: 2,
        sla_eligible_count: 2,
        success_count: 2,
        service_error_count: 0,
        user_error_count: 0,
        output_tokens: 100,
        avg_output_tps: 25,
        avg_first_byte_time_ms: 120,
        avg_response_time_ms: 1000,
        success_rate: 100,
      },
      {
        date: '2024-03-22',
        provider_id: 'provider-b',
        provider: 'Anthropic',
        request_count: 1,
        sla_eligible_count: 1,
        success_count: 1,
        service_error_count: 0,
        user_error_count: 0,
        output_tokens: 40,
        avg_output_tps: null,
        avg_first_byte_time_ms: 220,
        avg_response_time_ms: 1500,
        success_rate: 100,
      },
    ]

    const chart = buildProviderPerformanceChartData(timeline, 'avg_output_tps', providers)

    expect(chart.labels).toEqual(['2024-03-21', '2024-03-22'])
    expect(chart.datasets.map(dataset => dataset.label)).toEqual(['OpenAI', 'Anthropic'])
    expect(chart.datasets[0].data).toEqual([25, null])
    expect(chart.datasets[1].data).toEqual([null, null])
  })
})
