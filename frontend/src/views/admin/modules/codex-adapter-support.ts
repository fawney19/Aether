import type { GlobalModelResponse } from '@/api/global-models'
import type { CodexAdapterGlobalModelCompatibility } from '@/api/modules-codex-adapter'
import type {
  CodexAdapterCandidateConfig,
  CodexAdapterRouteConfig,
  CodexAdapterSchedulingMode,
} from '@/api/modules'

export interface CodexAdapterGlobalModelOption {
  value: string
  label: string
  compatible: boolean
  summary: string | null
}

export type CodexAdapterCompatibilityMap = Record<string, CodexAdapterGlobalModelCompatibility>

export function buildCodexAdapterCompatibilityMap(
  items: CodexAdapterGlobalModelCompatibility[],
): CodexAdapterCompatibilityMap {
  return Object.fromEntries(items.map((item) => [item.global_model, item]))
}

export function cloneCodexAdapterRoutes(
  routes: CodexAdapterRouteConfig[],
): CodexAdapterRouteConfig[] {
  return routes.map((route) => ({
    codex_model: route.codex_model,
    enabled: route.enabled,
    scheduling_mode: route.scheduling_mode,
    candidates: route.candidates.map((candidate) => ({ ...candidate })),
  }))
}

export function normalizeCodexAdapterSchedulingMode(
  value: string,
): CodexAdapterSchedulingMode {
  return value === 'sticky' || value === 'load_balance' ? value : 'priority'
}

export function toInteger(value: unknown, fallback: number): number {
  const parsed = Number.parseInt(String(value), 10)
  return Number.isNaN(parsed) ? fallback : parsed
}

export function toPositiveInteger(value: unknown, fallback: number): number {
  const parsed = Number.parseInt(String(value), 10)
  return Number.isNaN(parsed) || parsed <= 0 ? fallback : parsed
}

export function buildCodexAdapterGlobalModelOptions(
  globalModels: GlobalModelResponse[],
  routes: CodexAdapterRouteConfig[],
  compatibilityMap: CodexAdapterCompatibilityMap,
): CodexAdapterGlobalModelOption[] {
  const options = globalModels.map((model) => {
    const compatibility = compatibilityMap[model.name] ?? null
    return {
      value: model.name,
      label: model.display_name && model.display_name !== model.name
        ? `${model.display_name} (${model.name})`
        : model.name,
      compatible: compatibility?.compatible ?? true,
      summary: compatibility?.summary ?? null,
    }
  })

  const knownValues = new Set(options.map((option) => option.value))
  for (const route of routes) {
    for (const candidate of route.candidates) {
      const value = candidate.global_model.trim()
      if (!value || knownValues.has(value)) continue
      knownValues.add(value)
      const compatibility = compatibilityMap[value] ?? null
      options.push({
        value,
        label: `${value}（当前配置中，列表未收录）`,
        compatible: compatibility?.compatible ?? true,
        summary: compatibility?.summary ?? null,
      })
    }
  }

  return options
}

export function buildRouteCodexAdapterGlobalModelOptions(
  route: CodexAdapterRouteConfig,
  globalModelOptions: CodexAdapterGlobalModelOption[],
): CodexAdapterGlobalModelOption[] {
  const currentValues = route.candidates
    .map((candidate) => candidate.global_model.trim())
    .filter((value) => value.length > 0)
  if (currentValues.length === 0) return globalModelOptions

  const seen = new Set(globalModelOptions.map((option) => option.value))
  const routeOnly = currentValues
    .filter((value) => !seen.has(value))
    .map((value) => ({
      value,
      label: `${value}（当前配置中，列表未收录）`,
      compatible: true,
      summary: null,
    }))
  return [...globalModelOptions, ...routeOnly]
}

export function findCodexAdapterCompatibility(
  compatibilityMap: CodexAdapterCompatibilityMap,
  globalModel: string,
): CodexAdapterGlobalModelCompatibility | null {
  const normalized = globalModel.trim()
  return normalized ? compatibilityMap[normalized] ?? null : null
}

export function validateCodexAdapterCompatibleRoutes(
  routes: CodexAdapterRouteConfig[],
  compatibilityMap: CodexAdapterCompatibilityMap,
): string | null {
  for (const route of routes) {
    if (!route.enabled) continue
    for (const candidate of route.candidates) {
      if (!candidate.enabled) continue
      const compatibility = findCodexAdapterCompatibility(
        compatibilityMap,
        candidate.global_model,
      )
      if (!compatibility || compatibility.compatible) continue
      return `${route.codex_model || '未命名路由'} 的候选模型 ${candidate.global_model} 当前不可用：${compatibility.summary ?? '无法承接 Responses 请求'}`
    }
  }

  return null
}

export function reorderCodexAdapterCandidates(
  candidates: readonly CodexAdapterCandidateConfig[],
  fromIndex: number,
  toIndex: number,
): CodexAdapterCandidateConfig[] {
  const items = candidates.map((candidate, originalIndex) => ({
    candidate: { ...candidate },
    originalIndex,
    originalPriority: candidate.priority,
  }))

  if (
    fromIndex < 0
    || toIndex < 0
    || fromIndex >= items.length
    || toIndex >= items.length
    || fromIndex === toIndex
  ) {
    return items.map((item) => item.candidate)
  }

  const movedItem = items[fromIndex]
  const targetItem = items[toIndex]
  if (!movedItem || !targetItem) {
    return items.map((item) => item.candidate)
  }

  items.splice(fromIndex, 1)
  items.splice(toIndex, 0, movedItem)

  if (movedItem.originalPriority === targetItem.originalPriority) {
    return items.map((item) => item.candidate)
  }

  const groupNewPriority = new Map<number, number>()
  let currentPriority = 0

  items.forEach((item) => {
    if (item.originalIndex === movedItem.originalIndex) {
      item.candidate.priority = currentPriority
      currentPriority += 1
      return
    }

    const existingPriority = groupNewPriority.get(item.originalPriority)
    if (existingPriority != null) {
      item.candidate.priority = existingPriority
      return
    }

    groupNewPriority.set(item.originalPriority, currentPriority)
    item.candidate.priority = currentPriority
    currentPriority += 1
  })

  return items.map((item) => item.candidate)
}
