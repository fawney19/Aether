import { computed, ref } from 'vue'
import { getProvidersSummary } from '@/api/endpoints/providers'
import { getGlobalModels } from '@/api/global-models'
import { getProviderKeys } from '@/api/endpoints/keys'
import { adminApi } from '@/api/admin'
import { log } from '@/utils/logger'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types'
import type { GlobalModelResponse } from '@/api/global-models'
import type { EndpointAPIKey } from '@/api/endpoints/keys'

export function useUserAccessControlOptions() {
  const providers = ref<ProviderWithEndpointsSummary[]>([])
  const globalModels = ref<GlobalModelResponse[]>([])
  const apiFormats = ref<Array<{ value: string; label: string }>>([])
  const providerKeysByProvider = ref<Record<string, EndpointAPIKey[]>>({})
  const providerKeysLoading = ref<Record<string, boolean>>({})
  const providerKeysLoaded = ref<Record<string, boolean>>({})

  const providerOptions = computed(() =>
    providers.value.map((provider) => ({
      value: provider.id,
      label: provider.name,
    })),
  )
  const apiFormatOptions = computed(() =>
    apiFormats.value.map((format) => ({
      value: format.value,
      label: format.label,
    })),
  )
  const modelOptions = computed(() =>
    globalModels.value.map((model) => ({
      value: model.name,
      label: model.name,
    })),
  )

  async function loadAccessControlOptions(): Promise<void> {
    const [providersResponse, modelsData, formatsData] = await Promise.all([
      getProvidersSummary({ page_size: 9999 }),
      getGlobalModels({ limit: 1000, is_active: true }),
      adminApi.getApiFormats(),
    ])
    providers.value = providersResponse.items
    globalModels.value = modelsData.models || []
    apiFormats.value = formatsData.formats || []
  }

  async function loadProviderKeys(providerId: string): Promise<void> {
    if (providerKeysLoaded.value[providerId] || providerKeysLoading.value[providerId]) return
    providerKeysLoading.value = { ...providerKeysLoading.value, [providerId]: true }
    try {
      const keys = await getProviderKeys(providerId)
      // Keep disabled keys in the list: keys that are already selected but
      // became disabled must stay visible so the admin can uncheck them.
      providerKeysByProvider.value = {
        ...providerKeysByProvider.value,
        [providerId]: keys,
      }
      providerKeysLoaded.value = { ...providerKeysLoaded.value, [providerId]: true }
    } catch (err) {
      log.error('加载提供商 Key 失败:', err)
      providerKeysByProvider.value = { ...providerKeysByProvider.value, [providerId]: [] }
    } finally {
      const nextLoading = { ...providerKeysLoading.value }
      delete nextLoading[providerId]
      providerKeysLoading.value = nextLoading
    }
  }

  function clearProviderKeysCache(providerIds: string[]): void {
    const next: Record<string, EndpointAPIKey[]> = { ...providerKeysByProvider.value }
    const nextLoaded: Record<string, boolean> = { ...providerKeysLoaded.value }
    for (const providerId of providerIds) {
      delete next[providerId]
      delete nextLoaded[providerId]
    }
    providerKeysByProvider.value = next
    providerKeysLoaded.value = nextLoaded
  }

  return {
    providers,
    globalModels,
    apiFormats,
    providerOptions,
    apiFormatOptions,
    modelOptions,
    providerKeysByProvider,
    providerKeysLoading,
    loadAccessControlOptions,
    loadProviderKeys,
    clearProviderKeysCache,
  }
}
