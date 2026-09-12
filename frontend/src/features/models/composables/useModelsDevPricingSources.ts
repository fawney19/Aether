import { ref } from 'vue'

export interface ModelsDevPricingSource {
  provider_id: string
  provider_name: string
}

export const MODELS_DEV_PRICING_SOURCE_CONFIG_KEY = 'models_dev_pricing_source'

interface StoredModelsDevPricingSources {
  version: 1
  models: Record<string, ModelsDevPricingSource>
}

const STORAGE_KEY = 'aether:models-dev-pricing-sources:v1'
const LEGACY_STORAGE_KEY = 'aether:models-dev-pricing-preferences:v1'
const sources = ref<Record<string, ModelsDevPricingSource>>({})

function normalizePricingSource(value: unknown): ModelsDevPricingSource | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const source = value as Partial<ModelsDevPricingSource>
  const providerId = typeof source.provider_id === 'string' ? source.provider_id.trim() : ''
  const providerName = typeof source.provider_name === 'string' ? source.provider_name.trim() : ''
  if (!providerId || !providerName) return null
  return {
    provider_id: providerId,
    provider_name: providerName,
  }
}

/**
 * Reads the shared price-source record persisted with global_models.config.
 * localStorage remains only as a migration fallback for records created by
 * older Aether builds.
 */
export function getModelsDevPricingSourceFromConfig(
  config: Record<string, unknown> | null | undefined,
): ModelsDevPricingSource | null {
  if (!config || typeof config !== 'object' || Array.isArray(config)) return null
  return normalizePricingSource(config[MODELS_DEV_PRICING_SOURCE_CONFIG_KEY])
}

export function withModelsDevPricingSource(
  config: Record<string, unknown> | null | undefined,
  source: ModelsDevPricingSource,
): Record<string, unknown> {
  const normalizedSource = normalizePricingSource(source)
  if (!normalizedSource) return { ...(config ?? {}) }
  return {
    ...(config ?? {}),
    [MODELS_DEV_PRICING_SOURCE_CONFIG_KEY]: normalizedSource,
  }
}

export function modelsDevPricingSourcesEqual(
  left: ModelsDevPricingSource | null | undefined,
  right: ModelsDevPricingSource | null | undefined,
): boolean {
  return left?.provider_id.trim().toLowerCase() === right?.provider_id.trim().toLowerCase()
    && left?.provider_name.trim() === right?.provider_name.trim()
}

function parseStoredSources(key: string): Record<string, ModelsDevPricingSource> | null {
  try {
    const stored = JSON.parse(localStorage.getItem(key) || 'null') as unknown
    if (!stored || typeof stored !== 'object') return null
    const document = stored as Partial<StoredModelsDevPricingSources>
    if (document.version !== 1 || !document.models || typeof document.models !== 'object') return null

    const validSources: Record<string, ModelsDevPricingSource> = {}
    for (const [modelId, value] of Object.entries(document.models)) {
      const source = normalizePricingSource(value)
      if (source) validSources[modelId] = source
    }
    return validSources
  } catch {
    return null
  }
}

function writeStoredSources(value: Record<string, ModelsDevPricingSource>): boolean {
  try {
    const document: StoredModelsDevPricingSources = {
      version: 1,
      models: value,
    }
    localStorage.setItem(STORAGE_KEY, JSON.stringify(document))
    return true
  } catch {
    return false
  }
}

function readStoredSources(): Record<string, ModelsDevPricingSource> {
  if (typeof localStorage === 'undefined') return {}

  const currentSources = parseStoredSources(STORAGE_KEY)
  if (currentSources) {
    localStorage.removeItem(LEGACY_STORAGE_KEY)
    return currentSources
  }

  const legacySources = parseStoredSources(LEGACY_STORAGE_KEY)
  if (legacySources && writeStoredSources(legacySources)) {
    localStorage.removeItem(LEGACY_STORAGE_KEY)
  }
  return legacySources ?? {}
}

export function useModelsDevPricingSources() {
  sources.value = readStoredSources()

  function getLocalSource(modelId: string): ModelsDevPricingSource | null {
    return sources.value[modelId] ?? null
  }

  function getSource(
    modelId: string,
    config?: Record<string, unknown> | null,
  ): ModelsDevPricingSource | null {
    return getModelsDevPricingSourceFromConfig(config) ?? getLocalSource(modelId)
  }

  function setSource(modelId: string, source: ModelsDevPricingSource) {
    const normalizedSource = normalizePricingSource(source)
    if (!normalizedSource) return
    const nextSources = {
      ...sources.value,
      [modelId]: normalizedSource,
    }
    sources.value = nextSources
    writeStoredSources(nextSources)
  }

  return {
    getSource,
    getLocalSource,
    setSource,
  }
}
