import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/views/admin/PerformanceAnalysis.vue'),
  'utf8',
)

describe('PerformanceAnalysis progressive live data', () => {
  it('commits each live response before the full request group settles', () => {
    const liveDataBlock = source
      .split('async function loadLiveData')[1]
      ?.split('const errorTrendChartData')[0]
    expect(liveDataBlock).toBeTruthy()
    expect(liveDataBlock).toContain('function commitIfCurrent<T>')
    expect(liveDataBlock).toContain('if (requestId !== liveRequestId) return')
    expect(liveDataBlock).toContain('systemStatus.value = value')
    expect(liveDataBlock).toContain('resilienceStatus.value = value')
    expect(liveDataBlock).toContain('circuitHistory.value = value.items')
    expect(liveDataBlock).toContain('gatewayMetrics.value = value')
    expect(liveDataBlock).toContain('liveReady.value = true')

    expect(liveDataBlock).not.toContain('systemStatus.value = systemResult.value')
    expect(liveDataBlock).not.toContain('resilienceStatus.value = resilienceResult.value')
    expect(liveDataBlock).not.toContain('gatewayMetrics.value = metricsResult.value')
  })

  it('keeps group completion for loading state and error aggregation', () => {
    const liveDataBlock = source
      .split('async function loadLiveData')[1]
      ?.split('const errorTrendChartData')[0]
    expect(liveDataBlock).toContain('await Promise.allSettled([')
    expect(liveDataBlock).toContain("failedScopes.push('系统状态')")
    expect(liveDataBlock).toContain("failedScopes.push('韧性状态')")
    expect(liveDataBlock).toContain("failedScopes.push('熔断历史')")
    expect(liveDataBlock).toContain("failedScopes.push('网关指标')")
    expect(liveDataBlock).toContain('liveLoading.value = false')
    expect(liveDataBlock).toContain('liveRefreshing.value = false')
  })
})
