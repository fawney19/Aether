import { computed, onMounted, ref } from 'vue'
import { getGlobalModels, type GlobalModelResponse } from '@/api/global-models'
import { getCodexAdapterGlobalModelCompatibilities } from '@/api/modules-codex-adapter'
import {
  createDefaultCodexAdapterCandidateConfig,
  createDefaultCodexAdapterRouteConfig,
  modulesApi,
  serializeCodexAdapterConfig,
  validateCodexAdapterConfig,
  type CodexAdapterCandidateConfig,
  type CodexAdapterRouteConfig,
} from '@/api/modules'
import { useToast } from '@/composables/useToast'
import { useModuleStore } from '@/stores/modules'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import {
  buildCodexAdapterCompatibilityMap,
  buildRouteCodexAdapterGlobalModelOptions,
  cloneCodexAdapterRoutes,
  normalizeCodexAdapterSchedulingMode,
  reorderCodexAdapterCandidates,
  validateCodexAdapterCompatibleRoutes,
  type CodexAdapterCompatibilityMap,
  type CodexAdapterGlobalModelOption,
} from '../codex-adapter-support'

export function useCodexAdapterConfig() {
  const moduleStore = useModuleStore()
  const { success, error } = useToast()

  const loading = ref(false)
  const saving = ref(false)
  const routes = ref<CodexAdapterRouteConfig[]>([])
  const originalRoutes = ref<CodexAdapterRouteConfig[]>([])
  const globalModels = ref<GlobalModelResponse[]>([])
  const compatibilityMap = ref<CodexAdapterCompatibilityMap>({})
  const selectedRouteIndex = ref(-1)

  const moduleStatus = computed(() => moduleStore.modules.codex_adapter ?? null)
  const selectedRoute = computed(() => routes.value[selectedRouteIndex.value] ?? null)
  const selectedRouteGlobalModelOptions = computed<CodexAdapterGlobalModelOption[]>(() => {
    if (!selectedRoute.value) return []
    return buildRouteCodexAdapterGlobalModelOptions(
      selectedRoute.value,
      globalModels.value.map((model) => {
        const compatibility = compatibilityMap.value[model.name] ?? null
        return {
          value: model.name,
          label: model.display_name && model.display_name !== model.name
            ? `${model.display_name} (${model.name})`
            : model.name,
          compatible: compatibility?.compatible ?? true,
          summary: compatibility?.summary ?? null,
        }
      }),
    )
  })

  const statusText = computed(() => {
    const status = moduleStatus.value
    if (!status) return '模块状态读取中'
    if (!status.available) return '模块当前不可用'
    if (!status.enabled) return '模块未启用'
    if (!status.active) return '模块未激活'
    return '已启用，按当前路由规则调度'
  })

  const hasChanges = computed(() => (
    JSON.stringify(serializeCodexAdapterConfig(routes.value))
    !== JSON.stringify(serializeCodexAdapterConfig(originalRoutes.value))
  ))

  function syncSelectedRoute(preferredModel: string | null = null): void {
    if (routes.value.length === 0) {
      selectedRouteIndex.value = -1
      return
    }

    if (preferredModel) {
      const matchedIndex = routes.value.findIndex((route) => route.codex_model === preferredModel)
      if (matchedIndex >= 0) {
        selectedRouteIndex.value = matchedIndex
        return
      }
    }

    selectedRouteIndex.value = selectedRouteIndex.value < 0
      ? 0
      : Math.min(selectedRouteIndex.value, routes.value.length - 1)
  }

  async function loadCompatibilityMap(
    nextRoutes: readonly CodexAdapterRouteConfig[],
    nextGlobalModels: readonly GlobalModelResponse[],
  ): Promise<void> {
    const compatibilityNames = Array.from(new Set([
      ...nextGlobalModels.map((model) => model.name),
      ...nextRoutes.flatMap((route) => route.candidates.map((candidate) => candidate.global_model)),
    ]))

    if (compatibilityNames.length === 0) {
      compatibilityMap.value = {}
      return
    }

    try {
      const compatibilities = await getCodexAdapterGlobalModelCompatibilities(compatibilityNames)
      compatibilityMap.value = buildCodexAdapterCompatibilityMap(compatibilities)
    } catch (compatibilityError) {
      compatibilityMap.value = {}
      error(parseApiError(compatibilityError, '获取候选模型兼容性失败'))
      log.error('获取候选模型兼容性失败:', compatibilityError)
    }
  }

  async function loadPage(): Promise<void> {
    loading.value = true
    try {
      const previousSelectedModel = selectedRoute.value?.codex_model ?? null
      const [config, globalModelResponse] = await Promise.all([
        modulesApi.getCodexAdapterConfig(),
        getGlobalModels({ limit: 500 }, { cacheTtlMs: 60_000 }),
        moduleStore.fetchModules(),
      ])
      const nextRoutes = cloneCodexAdapterRoutes(config)
      routes.value = nextRoutes
      originalRoutes.value = cloneCodexAdapterRoutes(config)
      globalModels.value = globalModelResponse.models
      syncSelectedRoute(previousSelectedModel)
      await loadCompatibilityMap(nextRoutes, globalModelResponse.models)
    } catch (err) {
      error(parseApiError(err, '获取 Codex 适配器配置失败'))
      log.error('获取 Codex 适配器配置失败:', err)
    } finally {
      loading.value = false
    }
  }

  function selectRoute(routeIndex: number): void {
    selectedRouteIndex.value = routeIndex
  }

  function addRoute(): void {
    routes.value.push(createDefaultCodexAdapterRouteConfig())
    selectedRouteIndex.value = routes.value.length - 1
  }

  function removeRoute(routeIndex: number): void {
    routes.value.splice(routeIndex, 1)
    if (routes.value.length === 0) {
      selectedRouteIndex.value = -1
      return
    }
    if (selectedRouteIndex.value > routeIndex) {
      selectedRouteIndex.value -= 1
      return
    }
    if (selectedRouteIndex.value === routeIndex) {
      selectedRouteIndex.value = Math.min(routeIndex, routes.value.length - 1)
      return
    }
    syncSelectedRoute()
  }

  function updateRoute(routeIndex: number, patch: Partial<CodexAdapterRouteConfig>): void {
    const currentRoute = routes.value[routeIndex]
    if (!currentRoute) return
    routes.value[routeIndex] = {
      ...currentRoute,
      ...patch,
      scheduling_mode: patch.scheduling_mode
        ? normalizeCodexAdapterSchedulingMode(String(patch.scheduling_mode))
        : currentRoute.scheduling_mode,
    }
  }

  function addCandidate(routeIndex: number): void {
    const route = routes.value[routeIndex]
    if (!route) return
    route.candidates.push(createDefaultCodexAdapterCandidateConfig())
  }

  function updateCandidate(
    routeIndex: number,
    candidateIndex: number,
    patch: Partial<CodexAdapterCandidateConfig>,
  ): void {
    const route = routes.value[routeIndex]
    const currentCandidate = route?.candidates[candidateIndex]
    if (!route || !currentCandidate) return
    route.candidates[candidateIndex] = {
      ...currentCandidate,
      ...patch,
    }
  }

  function removeCandidate(routeIndex: number, candidateIndex: number): void {
    const route = routes.value[routeIndex]
    if (!route) return
    route.candidates.splice(candidateIndex, 1)
  }

  function moveCandidate(routeIndex: number, candidateIndex: number, direction: -1 | 1): void {
    const route = routes.value[routeIndex]
    if (!route) return
    const nextIndex = candidateIndex + direction
    if (nextIndex < 0 || nextIndex >= route.candidates.length) return
    route.candidates = reorderCodexAdapterCandidates(route.candidates, candidateIndex, nextIndex)
  }

  async function saveConfig(): Promise<void> {
    const validationError = validateCodexAdapterConfig(routes.value)
    if (validationError) {
      error(validationError)
      return
    }

    const compatibilityError = validateCodexAdapterCompatibleRoutes(
      routes.value,
      compatibilityMap.value,
    )
    if (compatibilityError) {
      error(compatibilityError)
      return
    }

    saving.value = true
    try {
      const previousSelectedModel = selectedRoute.value?.codex_model ?? null
      const saved = await modulesApi.updateCodexAdapterConfig(routes.value)
      routes.value = cloneCodexAdapterRoutes(saved)
      originalRoutes.value = cloneCodexAdapterRoutes(saved)
      syncSelectedRoute(previousSelectedModel)
      success('Codex 适配器配置已保存')
    } catch (err) {
      error(parseApiError(err, '保存 Codex 适配器配置失败'))
      log.error('保存 Codex 适配器配置失败:', err)
    } finally {
      saving.value = false
    }
  }

  onMounted(() => {
    void loadPage()
  })

  return {
    addCandidate,
    addRoute,
    globalModels,
    hasChanges,
    loadPage,
    loading,
    moveCandidate,
    removeCandidate,
    removeRoute,
    routes,
    saveConfig,
    saving,
    selectRoute,
    selectedRoute,
    selectedRouteGlobalModelOptions,
    selectedRouteIndex,
    statusText,
    updateCandidate,
    updateRoute,
  }
}
