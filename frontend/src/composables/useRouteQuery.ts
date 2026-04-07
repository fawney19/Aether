import { useRoute, useRouter, type LocationQuery, type LocationQueryValue } from 'vue-router'

type QueryValue = LocationQueryValue | LocationQueryValue[]

function normalizeQueryValue(value: QueryValue): string | undefined {
  if (Array.isArray(value)) {
    return value.length > 0 ? (value[value.length - 1] ?? undefined) : undefined
  }
  return typeof value === 'string' ? value : undefined
}

function queriesEqual(left: LocationQuery, right: LocationQuery): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)])
  for (const key of keys) {
    if (normalizeQueryValue(left[key]) !== normalizeQueryValue(right[key])) {
      return false
    }
  }
  return true
}

export function useRouteQuery() {
  const route = useRoute()
  const router = useRouter()

  function getQueryValue(key: string): string | undefined {
    return normalizeQueryValue(route.query[key])
  }

  function patchQuery(patch: Record<string, string | undefined | null>) {
    const next: LocationQuery = { ...route.query }
    for (const [key, value] of Object.entries(patch)) {
      if (value == null || value.trim() === '') {
        delete next[key]
      } else {
        next[key] = value
      }
    }
    if (queriesEqual(route.query, next)) return
    void router.replace({ query: next }).catch(() => {})
  }

  return { route, router, getQueryValue, patchQuery }
}
