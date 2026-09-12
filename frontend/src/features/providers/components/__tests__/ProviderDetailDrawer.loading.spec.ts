import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/features/providers/components/ProviderDetailDrawer.vue'),
  'utf8',
)

describe('ProviderDetailDrawer loading priorities', () => {
  it('plays the drawer transition when the lazily mounted component first appears', () => {
    const transitionTemplate = source
      .split('<Transition')[1]
      ?.split('>')[0]

    expect(transitionTemplate).toContain('name="drawer"')
    expect(transitionTemplate).toContain('appear')
    expect(source).toContain('class="drawer-panel relative')
    expect(source).toContain('.drawer-enter-from .drawer-panel')
    expect(source).not.toContain('.drawer-enter-from .relative')
  })

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

  it('shows the provider-wide key total in the paginated footer', () => {
    expect(source).toContain("{{ legacyT('共') }} {{ providerKeysTotal }}")
    expect(source).not.toContain("{{ legacyT('共') }} {{ allKeys.length }}")
  })

  it('does not mount closed child dialogs or blur the page behind the drawer', () => {
    expect(source).toContain('class="absolute inset-0 bg-black/30"')
    expect(source).not.toContain('backdrop-blur-sm')
    expect(source).toContain('v-if="provider && open && endpointDialogOpen"')
    expect(source).toContain('v-if="open && keyFormDialogOpen"')
    expect(source).toContain('v-if="open && oauthAccountDialogOpen && provider"')
    expect(source).toContain('v-if="open && oauthKeyEditDialogOpen"')
    expect(source).toContain('v-if="open && keyPermissionsDialogOpen"')
    expect(source).toContain('v-if="open && modelFormDialogOpen && provider"')
    expect(source).toContain('v-if="open && batchAssignDialogOpen && provider"')
    expect(source).toContain('v-if="open && failoverRulesDialogOpen"')
  })

  it('marks a transport-error stop policy as a configured failover rule', () => {
    expect(source).toContain('rules.stop_on_transport_errors === true')
  })
})
