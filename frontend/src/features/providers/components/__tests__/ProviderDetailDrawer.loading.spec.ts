import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/features/providers/components/ProviderDetailDrawer.vue'),
  'utf8',
)

describe('ProviderDetailDrawer loading priorities', () => {
  it('loads mapping preview after first-screen provider data', () => {
    const openWatcher = source
      .split('// 合并监听 providerId 和 open')[1]
      ?.split('} else if (!newOpen && oldOpen)')[0]

    expect(openWatcher).toBeTruthy()
    expect(openWatcher).toContain('const endpointsPromise = loadEndpoints()')
    expect(openWatcher).toContain('endpointsPromise.then(() => {')
    expect(openWatcher).toContain('if (!props.open || props.providerId !== newId) return')
    expect(openWatcher).toContain('void loadMappingPreview()')

    const beforeEndpoints = openWatcher?.split('const endpointsPromise = loadEndpoints()')[0]
    expect(beforeEndpoints).not.toContain('loadMappingPreview()')
  })

  it('keeps model and mapping loading states independent', () => {
    expect(source).toContain(':loading="loadingProviderModels"')
    expect(source).toContain(':loading="loadingProviderMappingPreview"')
    expect(source).not.toContain(':loading="loadingProviderModels || loadingProviderKeys"')
    expect(source).not.toContain(':loading="loadingProviderEndpoints || loadingProviderKeys || loadingProviderModels || loadingProviderMappingPreview"')
  })
})
