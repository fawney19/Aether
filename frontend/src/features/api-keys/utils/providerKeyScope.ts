/**
 * Provider-scoped channel key allowlist helpers shared by the standalone API
 * key dialog and the user group dialog.
 *
 * Backend contract: `allowed_provider_key_ids` is `{ "<provider_id>":
 * ["<provider_api_keys.id>", ...] } | null`. `null` (or an empty object)
 * means "no key-level restriction" — every active key of every allowed
 * provider is usable.
 */

export type ProviderKeyScope = Record<string, string[]>

/** Normalizes a scope object: trims, dedups, sorts; drops empty entries and
 *  returns `null` when nothing restricts. */
export function normalizeProviderKeyScope(
  scope: ProviderKeyScope | null | undefined,
): ProviderKeyScope | null {
  if (!scope) return null
  const result: ProviderKeyScope = {}
  for (const [providerId, keyIds] of Object.entries(scope)) {
    const unique = [...new Set((keyIds ?? []).map((id) => id.trim()).filter(Boolean))].sort()
    if (unique.length > 0) result[providerId] = unique
  }
  return Object.keys(result).length > 0 ? result : null
}

/** Builds the editor checkbox state from a backend value (empty object when
 *  the backend value is absent). */
export function providerKeyScopeFromApi(
  scope: Record<string, string[]> | null | undefined,
): ProviderKeyScope {
  return scope ? { ...scope } : {}
}

/** Builds the submit payload. When providers are unrestricted the scope must
 *  be `null` (the backend rejects key scopes without a provider allowlist). */
export function providerKeyScopeForSubmit(
  scope: ProviderKeyScope,
  providersUnrestricted: boolean,
): Record<string, string[]> | null {
  if (providersUnrestricted) return null
  return normalizeProviderKeyScope(scope)
}

/** Drops scope entries whose provider is no longer selected. */
export function restrictProviderKeyScopeToProviders(
  scope: ProviderKeyScope,
  providers: string[],
): ProviderKeyScope {
  const allowed = new Set(providers)
  const result: ProviderKeyScope = {}
  for (const [providerId, keyIds] of Object.entries(scope)) {
    if (allowed.has(providerId) && keyIds.length > 0) {
      result[providerId] = [...keyIds]
    }
  }
  return result
}

/** Summarizes how many keys are selected (for display). */
export function providerKeyScopeSelectedCount(scope: ProviderKeyScope): number {
  return Object.values(scope).reduce((total, keyIds) => total + keyIds.length, 0)
}
