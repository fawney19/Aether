import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/views/admin/CacheMonitoring.vue'),
  'utf8',
)

describe('CacheMonitoring analysis loading', () => {
  it('observes the TTL section against the app scroll container', () => {
    expect(source).toContain('ref="analysisSectionRef"')
    expect(source).toContain("target.closest('.app-shell__content')")
    expect(source).toContain("rootMargin: '600px 0px'")
  })

  it('keeps TTL and hit analysis out of the initial request group', () => {
    const mountedBlock = source
      .split('onMounted(() => {')[1]
      ?.split('onBeforeUnmount')[0]

    expect(mountedBlock).toBeTruthy()
    expect(mountedBlock).toContain('const initialLoad = refreshData()')
    expect(mountedBlock).not.toContain('refreshAnalysis()')
    expect(source).toContain('const waitForInitialData = initialDataPromise ?? Promise.resolve()')
  })

  it('cleans up the observer when the page unmounts', () => {
    const unmountedBlock = source.split('onBeforeUnmount(() => {')[1]

    expect(unmountedBlock).toBeTruthy()
    expect(unmountedBlock).toContain('stopAnalysisObserver?.()')
  })
})
