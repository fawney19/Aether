import apiClient from './client'

export interface CodexAdapterGlobalModelCompatibility {
  global_model: string
  compatible: boolean
  reasons: string[]
  summary: string | null
}

function normalizeCompatibility(value: unknown): CodexAdapterGlobalModelCompatibility | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const item = value as Record<string, unknown>
  const globalModel = typeof item.global_model === 'string'
    ? item.global_model.trim()
    : ''
  if (!globalModel) return null

  const reasons = Array.isArray(item.reasons)
    ? item.reasons
      .filter((reason): reason is string => typeof reason === 'string' && reason.trim().length > 0)
      .map((reason) => reason.trim())
    : []
  const summary = typeof item.summary === 'string' && item.summary.trim()
    ? item.summary.trim()
    : null

  return {
    global_model: globalModel,
    compatible: item.compatible === true,
    reasons,
    summary,
  }
}

export async function getCodexAdapterGlobalModelCompatibilities(
  globalModels: string[],
): Promise<CodexAdapterGlobalModelCompatibility[]> {
  const names = Array.from(
    new Set(
      globalModels
        .map((name) => name.trim())
        .filter((name) => name.length > 0),
    ),
  )
  if (names.length === 0) return []

  const query = new URLSearchParams()
  for (const name of names) {
    query.append('global_model', name)
  }

  const response = await apiClient.get<{ items?: unknown }>(
    `/api/admin/models/global/codex-adapter-compatibility?${query.toString()}`,
  )
  const items = Array.isArray(response.data.items) ? response.data.items : []
  return items
    .map(normalizeCompatibility)
    .filter((item): item is CodexAdapterGlobalModelCompatibility => item !== null)
}
