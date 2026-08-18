import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, type App } from 'vue'

const apiMocks = vi.hoisted(() => ({
  getProvidersSummary: vi.fn(),
  getGlobalModels: vi.fn(),
  getProviderKeys: vi.fn(),
  getApiFormats: vi.fn(),
  logError: vi.fn(),
}))

vi.mock('@/api/endpoints/providers', () => ({
  getProvidersSummary: apiMocks.getProvidersSummary,
}))
vi.mock('@/api/global-models', () => ({
  getGlobalModels: apiMocks.getGlobalModels,
}))
vi.mock('@/api/endpoints/keys', () => ({
  getProviderKeys: apiMocks.getProviderKeys,
}))
vi.mock('@/api/admin', () => ({
  adminApi: {
    getApiFormats: apiMocks.getApiFormats,
  },
}))
vi.mock('@/utils/logger', () => ({
  log: {
    error: apiMocks.logError,
  },
}))

import { useUserAccessControlOptions } from '../useUserAccessControlOptions'

interface MountedOptions {
  app: App
  root: HTMLElement
}

const mountedOptions: MountedOptions[] = []

type AccessControlOptions = ReturnType<typeof useUserAccessControlOptions>

function mountOptions(): AccessControlOptions {
  let options!: AccessControlOptions
  const root = document.createElement('div')
  document.body.appendChild(root)
  const host = defineComponent({
    setup() {
      options = useUserAccessControlOptions()
      return () => null
    },
  })
  const app = createApp(host)
  app.mount(root)
  mountedOptions.push({ app, root })
  return options
}

const activeKey = {
  id: 'key-active',
  name: 'Active key',
  api_key_masked: 'sk-active',
  is_active: true,
}
const disabledKey = {
  id: 'key-disabled',
  name: 'Disabled key',
  api_key_masked: 'sk-disabled',
  is_active: false,
}

beforeEach(() => {
  apiMocks.getProvidersSummary.mockReset().mockResolvedValue({ items: [] })
  apiMocks.getGlobalModels.mockReset().mockResolvedValue({ models: [] })
  apiMocks.getProviderKeys.mockReset()
  apiMocks.getApiFormats.mockReset().mockResolvedValue({ formats: [] })
  apiMocks.logError.mockReset()
})

afterEach(() => {
  for (const mounted of mountedOptions.splice(0)) {
    mounted.app.unmount()
    mounted.root.remove()
  }
})

describe('useUserAccessControlOptions provider key cache', () => {
  it('keeps disabled keys visible and reloads after the provider is cleared', async () => {
    apiMocks.getProviderKeys.mockResolvedValueOnce([activeKey, disabledKey])
    const options = mountOptions()

    await options.loadProviderKeys('provider-1')
    await options.loadProviderKeys('provider-1')

    expect(apiMocks.getProviderKeys).toHaveBeenCalledOnce()
    expect(options.providerKeysByProvider.value['provider-1']).toEqual([activeKey, disabledKey])

    options.clearProviderKeysCache(['provider-1'])
    apiMocks.getProviderKeys.mockResolvedValueOnce([activeKey])
    await options.loadProviderKeys('provider-1')

    expect(apiMocks.getProviderKeys).toHaveBeenCalledTimes(2)
    expect(options.providerKeysByProvider.value['provider-1']).toEqual([activeKey])
  })

  it('does not mark a failed request as loaded and permits retry', async () => {
    apiMocks.getProviderKeys
      .mockRejectedValueOnce(new Error('temporary provider key failure'))
      .mockResolvedValueOnce([activeKey])
    const options = mountOptions()

    await options.loadProviderKeys('provider-1')
    await options.loadProviderKeys('provider-1')

    expect(apiMocks.getProviderKeys).toHaveBeenCalledTimes(2)
    expect(options.providerKeysByProvider.value['provider-1']).toEqual([activeKey])
    expect(apiMocks.logError).toHaveBeenCalledOnce()
  })
})
