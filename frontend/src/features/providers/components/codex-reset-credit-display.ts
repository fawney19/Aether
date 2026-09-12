import type {
  CodexUpstreamMetadata,
  QuotaResetCreditSnapshot,
  QuotaResetCreditsSnapshot,
} from '@/api/endpoints/types'

function codexQuotaUpdatedAt(display: CodexUpstreamMetadata | null | undefined): number | null {
  const updatedAt = Number(display?.updated_at)
  return Number.isFinite(updatedAt) ? updatedAt : null
}

export function mergeCodexQuotaDisplays(
  snapshotDisplay: CodexUpstreamMetadata | null | undefined,
  metadataDisplay: CodexUpstreamMetadata | null | undefined,
): CodexUpstreamMetadata | null {
  if (!snapshotDisplay) return metadataDisplay ?? null
  if (!metadataDisplay) return snapshotDisplay

  const snapshotUpdatedAt = codexQuotaUpdatedAt(snapshotDisplay)
  const metadataUpdatedAt = codexQuotaUpdatedAt(metadataDisplay)
  const metadataIsNewer = metadataUpdatedAt !== null
    && (snapshotUpdatedAt === null || metadataUpdatedAt > snapshotUpdatedAt)
  const preferred = metadataIsNewer ? metadataDisplay : snapshotDisplay
  const fallback = metadataIsNewer ? snapshotDisplay : metadataDisplay
  const resetCredits = preferred.reset_credits || fallback.reset_credits
    ? {
        ...fallback.reset_credits,
        ...preferred.reset_credits,
      }
    : undefined

  return {
    ...fallback,
    ...preferred,
    ...(resetCredits ? { reset_credits: resetCredits } : {}),
  }
}

export interface CodexResetCreditDisplayItem {
  id?: string | null
  displayKey: string
  expiresAt?: number | null
  remainingSeconds: number
  title: string
}

interface CodexResetCreditDisplayCandidate {
  id?: string | null
  expiresAt?: number | null
  remainingSeconds: number
}

export function getCodexResetCreditAvailableCount(
  snapshot: QuotaResetCreditsSnapshot | null | undefined,
): number | null {
  const count = snapshot?.available_count
  return typeof count === 'number' && Number.isFinite(count) && count >= 0 ? count : null
}

export function formatCodexResetCreditCount(count: number | null | undefined): string {
  return `共 ${count ?? 0} 次机会`
}

interface CodexResetCreditCrypto {
  randomUUID?: () => string
  getRandomValues: (array: Uint8Array) => Uint8Array
}

interface CodexResetCreditPendingStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

const CODEX_RESET_CREDIT_PENDING_STORAGE_PREFIX = 'aether:codex-reset-credit-pending:v2:'
const CODEX_RESET_CREDIT_LEGACY_PENDING_STORAGE_PREFIX = 'aether:codex-reset-credit-pending:v1:'

interface CodexResetCreditPendingAttempt {
  idempotencyKey: string
  credentialGeneration: string | null
}

const pendingCodexResetCreditAttempts = new Map<string, CodexResetCreditPendingAttempt | null>()

const CODEX_RESET_CREDIT_TERMINAL_OUTCOMES = new Set([
  'reset',
  'already_redeemed',
  'nothing_to_reset',
  'no_credit',
  'historical_replay',
])

function codexResetCreditPendingStorageKey(keyId: string): string {
  return `${CODEX_RESET_CREDIT_PENDING_STORAGE_PREFIX}${keyId.trim()}`
}

function codexResetCreditLegacyPendingStorageKey(keyId: string): string {
  return `${CODEX_RESET_CREDIT_LEGACY_PENDING_STORAGE_PREFIX}${keyId.trim()}`
}

function normalizeCodexCredentialGeneration(value: string | null | undefined): string | null {
  const normalized = value?.trim()
  return normalized ? normalized : null
}

function parseCodexResetCreditPendingAttempt(value: string): CodexResetCreditPendingAttempt | null {
  try {
    const parsed = JSON.parse(value) as unknown
    if (typeof parsed !== 'object' || parsed === null) return null
    const record = parsed as Record<string, unknown>
    const idempotencyKey = typeof record.idempotencyKey === 'string'
      ? record.idempotencyKey.trim()
      : ''
    const generation = record.credentialGeneration
    if (!idempotencyKey || idempotencyKey.length > 256) return null
    if (generation !== null && typeof generation !== 'string') return null
    const credentialGeneration = normalizeCodexCredentialGeneration(generation)
    if (typeof generation === 'string' && credentialGeneration === null) return null
    return { idempotencyKey, credentialGeneration }
  } catch {
    return null
  }
}

function resolveCodexResetCreditPendingStorage(
  storage: CodexResetCreditPendingStorage | undefined,
): CodexResetCreditPendingStorage | undefined {
  if (storage) return storage
  try {
    return globalThis.sessionStorage
  } catch {
    return undefined
  }
}

export function isCodexResetCreditTerminalOutcome(outcome: string): boolean {
  return CODEX_RESET_CREDIT_TERMINAL_OUTCOMES.has(outcome)
}

export function getCodexResetCreditReservationIdempotencyKey(
  display: CodexUpstreamMetadata | null | undefined,
): string | null {
  const value = display?.account_quota_reset_reservation?.idempotency_key?.trim()
  return value && value.length <= 256 ? value : null
}

export function createCodexResetCreditIdempotencyKey(
  cryptoSource: CodexResetCreditCrypto | undefined = globalThis.crypto,
): string {
  const randomUUID = cryptoSource?.randomUUID?.bind(cryptoSource)
  if (randomUUID) return randomUUID()
  if (!cryptoSource) {
    throw new Error('浏览器不支持安全随机数，无法生成幂等 ID')
  }

  const bytes = cryptoSource.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 0x0f) | 0x40
  bytes[8] = (bytes[8] & 0x3f) | 0x80
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

export function readPendingCodexResetCreditIdempotencyKey(
  keyId: string,
  expectedCredentialGeneration: string | null,
  storage?: CodexResetCreditPendingStorage,
): string | null {
  const storageKey = codexResetCreditPendingStorageKey(keyId)
  const expectedGeneration = normalizeCodexCredentialGeneration(expectedCredentialGeneration)
  const resolvedStorage = resolveCodexResetCreditPendingStorage(storage)
  try {
    resolvedStorage?.removeItem(codexResetCreditLegacyPendingStorageKey(keyId))
  } catch {
    // Legacy values are never read, so failed cleanup cannot replay them.
  }
  if (pendingCodexResetCreditAttempts.has(storageKey)) {
    const attempt = pendingCodexResetCreditAttempts.get(storageKey)
    if (attempt?.credentialGeneration === expectedGeneration) return attempt.idempotencyKey
    pendingCodexResetCreditAttempts.set(storageKey, null)
    try {
      resolvedStorage?.removeItem(storageKey)
    } catch {
      // The in-memory tombstone still prevents a stale generation from replaying.
    }
    return null
  }
  try {
    const value = resolvedStorage?.getItem(storageKey)?.trim()
    const attempt = value ? parseCodexResetCreditPendingAttempt(value) : null
    if (attempt?.credentialGeneration === expectedGeneration) {
      pendingCodexResetCreditAttempts.set(storageKey, attempt)
      return attempt.idempotencyKey
    }
    pendingCodexResetCreditAttempts.set(storageKey, null)
    if (value) resolvedStorage?.removeItem(storageKey)
  } catch {
    // Fall through to the in-memory copy when browser storage is unavailable.
  }
  return null
}

export function rememberPendingCodexResetCreditIdempotencyKey(
  keyId: string,
  idempotencyKey: string,
  credentialGeneration: string | null,
  storage?: CodexResetCreditPendingStorage,
): void {
  const normalized = idempotencyKey.trim()
  if (!normalized || normalized.length > 256) return
  const storageKey = codexResetCreditPendingStorageKey(keyId)
  const attempt: CodexResetCreditPendingAttempt = {
    idempotencyKey: normalized,
    credentialGeneration: normalizeCodexCredentialGeneration(credentialGeneration),
  }
  pendingCodexResetCreditAttempts.set(storageKey, attempt)
  try {
    resolveCodexResetCreditPendingStorage(storage)?.setItem(storageKey, JSON.stringify(attempt))
  } catch {
    // The in-memory copy still permits retrying the same request in this page session.
  }
}

export function clearPendingCodexResetCreditIdempotencyKey(
  keyId: string,
  storage?: CodexResetCreditPendingStorage,
): void {
  const storageKey = codexResetCreditPendingStorageKey(keyId)
  pendingCodexResetCreditAttempts.set(storageKey, null)
  try {
    const resolvedStorage = resolveCodexResetCreditPendingStorage(storage)
    resolvedStorage?.removeItem(storageKey)
    resolvedStorage?.removeItem(codexResetCreditLegacyPendingStorageKey(keyId))
  } catch {
    // Ignore unavailable browser storage after a terminal server response.
  }
}

export function clearPendingCodexResetCreditIdempotencyKeyForOutcome(
  keyId: string,
  outcome: string,
  storage?: CodexResetCreditPendingStorage,
): boolean {
  if (!isCodexResetCreditTerminalOutcome(outcome)) return false
  clearPendingCodexResetCreditIdempotencyKey(keyId, storage)
  return true
}

function codexResetCreditRemainingSeconds(
  item: QuotaResetCreditSnapshot,
  snapshot: QuotaResetCreditsSnapshot,
  nowUnixSecs: number,
): number | null {
  if (typeof item.expires_at === 'number' && Number.isFinite(item.expires_at)) {
    return Math.max(item.expires_at - nowUnixSecs, 0)
  }
  if (typeof item.remaining_seconds === 'number' && Number.isFinite(item.remaining_seconds)) {
    const updatedAt = snapshot.updated_at
    const elapsed = typeof updatedAt === 'number' && Number.isFinite(updatedAt)
      ? Math.max(nowUnixSecs - updatedAt, 0)
      : 0
    return Math.max(item.remaining_seconds - elapsed, 0)
  }
  return null
}

function codexResetCreditStatusIsDisplayable(item: QuotaResetCreditSnapshot): boolean {
  const status = item.status?.trim().toLowerCase()
  return !status || status === 'available' || status === 'active'
}

export function getVisibleCodexResetCreditItems(
  snapshot: QuotaResetCreditsSnapshot | null | undefined,
  nowUnixSecs = Math.floor(Date.now() / 1000),
  limit = 5,
): CodexResetCreditDisplayItem[] {
  const credits = snapshot?.credits
  if (!snapshot || !Array.isArray(credits)) return []

  return credits
    .map((item): CodexResetCreditDisplayCandidate | null => {
      if (!codexResetCreditStatusIsDisplayable(item)) return null
      const remainingSeconds = codexResetCreditRemainingSeconds(item, snapshot, nowUnixSecs)
      if (remainingSeconds === null || remainingSeconds <= 0) return null
      return {
        id: item.id,
        expiresAt: nowUnixSecs + remainingSeconds,
        remainingSeconds,
      } satisfies CodexResetCreditDisplayCandidate
    })
    .filter((item): item is CodexResetCreditDisplayCandidate => item !== null)
    .sort((a, b) => a.remainingSeconds - b.remainingSeconds)
    .slice(0, limit)
    .map((item, index) => {
      const displayKey = `Key-${index + 1}`
      return {
        ...item,
        displayKey,
        title: `Codex 重置机会 ${displayKey}`,
      } satisfies CodexResetCreditDisplayItem
    })
}

export function formatCodexResetCreditExpiresAt(expiresAt: number | null | undefined): string {
  if (typeof expiresAt !== 'number' || !Number.isFinite(expiresAt)) return '-'
  const date = new Date(expiresAt * 1000)
  if (Number.isNaN(date.getTime())) return '-'

  const pad = (value: number) => String(value).padStart(2, '0')
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}
