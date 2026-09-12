import type { QuotaWindowSnapshot } from '@/api/endpoints/types'

export interface AntigravityQuotaSortableItem {
  model: string
  label: string
  remainingPercent: number
  resetSeconds: number | null
  detail?: string
}

type AntigravityQuotaPeriodMessageKey =
  | 'providers.antigravity.quota.period.weekly'
  | 'providers.antigravity.quota.period.fiveHours'

type AntigravityQuotaTranslator = (key: AntigravityQuotaPeriodMessageKey) => string

const ANTIGRAVITY_MODEL_LABELS: Record<string, string> = {
  'gemini-pro-agent': 'Gemini 3.1 Pro (High)',
  'gemini-3.1-pro-high': 'Gemini 3.1 Pro (High)',
  'gemini-3.1-pro-low': 'Gemini 3.1 Pro (Low)',
  'gemini-3-flash-agent': 'Gemini 3.5 Flash (High)',
  'gemini-3.5-flash-low': 'Gemini 3.5 Flash (Medium)',
  'gemini-3.5-flash-extra-low': 'Gemini 3.5 Flash (Low)',
  'claude-opus-4-6-thinking': 'Claude Opus 4.6 (Thinking)',
  'claude-sonnet-4-6': 'Claude Sonnet 4.6 (Thinking)',
  'claude-sonnet-4-6-thinking': 'Claude Sonnet 4.6 (Thinking)',
  'gemini-3.1-flash-image': 'Gemini 3.1 Flash Image',
  'gemini-3.1-flash-lite': 'Gemini 3.1 Flash Lite',
  'gemini-3-flash': 'Gemini 3 Flash',
  'gemini-2.5-pro': 'Gemini 2.5 Pro',
  'gemini-2.5-flash-thinking': 'Gemini 3.1 Flash Lite',
  'gemini-2.5-flash': 'Gemini 3.1 Flash Lite',
  'gemini-2.5-flash-lite': 'Gemini 3.1 Flash Lite',
  'gpt-oss-120b-medium': 'GPT-OSS 120B (Medium)',
  'tab_flash_lite_preview': 'Tab Flash Lite Preview',
  'tab_jump_flash_lite_preview': 'Tab Jump Flash Lite Preview',
  'models/proactive-observer': 'Proactive Observer',
}

const ANTIGRAVITY_MODEL_PRIORITY: Record<string, number> = {
  'claude-opus-4-6-thinking': 10,
  'claude-sonnet-4-6': 20,
  'claude-sonnet-4-6-thinking': 25,
  'gemini-pro-agent': 30,
  'gemini-3.1-pro-high': 35,
  'gemini-3.1-pro-low': 40,
  'gemini-3-flash-agent': 50,
  'gemini-3.5-flash-low': 60,
  'gemini-3.5-flash-extra-low': 70,
  'gemini-3.1-flash-image': 80,
  'gemini-3.1-flash-lite': 90,
  'gemini-3-flash': 180,
  'gemini-2.5-pro': 300,
  'gemini-2.5-flash-thinking': 310,
  'gemini-2.5-flash': 320,
  'gemini-2.5-flash-lite': 330,
  'gpt-oss-120b-medium': 700,
  'models/proactive-observer': 780,
  'tab_flash_lite_preview': 800,
  'tab_jump_flash_lite_preview': 810,
}

export function isOpaqueAntigravityQuotaIdentifier(value: string): boolean {
  return value.trim().startsWith('RateLimitResetCredit_')
}

export function resolveAntigravityQuotaLabel(
  model: string,
  rawLabel: unknown,
  opaqueDisplayIndex: { value: number },
): string {
  const normalizedModel = model.trim()
  const canonical = ANTIGRAVITY_MODEL_LABELS[normalizedModel]
  if (canonical) return canonical

  const candidate = String(rawLabel || '').trim()
  if (candidate && !isOpaqueAntigravityQuotaIdentifier(candidate)) return candidate
  if (isOpaqueAntigravityQuotaIdentifier(normalizedModel) || (candidate && isOpaqueAntigravityQuotaIdentifier(candidate))) {
    const label = `Key-${opaqueDisplayIndex.value}`
    opaqueDisplayIndex.value += 1
    return label
  }
  return candidate || normalizedModel
}

function getAntigravityModelPriority(model: string): number {
  const normalizedModel = model.trim()
  const explicit = ANTIGRAVITY_MODEL_PRIORITY[normalizedModel]
  if (explicit !== undefined) return explicit
  if (normalizedModel.startsWith('claude-')) return 30
  if (normalizedModel.startsWith('gemini-3.')) return 200
  if (normalizedModel.startsWith('gemini-2.')) return 390
  if (normalizedModel.startsWith('gemini-')) return 490
  if (normalizedModel.startsWith('gpt-oss-')) return 700
  if (normalizedModel.startsWith('models/')) return 780
  if (normalizedModel.startsWith('tab_')) return 800
  if (normalizedModel.startsWith('chat_')) return 900
  if (isOpaqueAntigravityQuotaIdentifier(normalizedModel)) return 950
  return 850
}

export function compareAntigravityQuotaItems<T extends AntigravityQuotaSortableItem>(
  a: T,
  b: T,
): number {
  return (getAntigravityModelPriority(a.model) - getAntigravityModelPriority(b.model))
    || ((a.resetSeconds ?? Number.POSITIVE_INFINITY) - (b.resetSeconds ?? Number.POSITIVE_INFINITY))
    || (a.remainingPercent - b.remainingPercent)
    || a.label.localeCompare(b.label)
    || a.model.localeCompare(b.model)
}

export function dedupeAntigravityQuotaItemsByLabel<T extends AntigravityQuotaSortableItem>(
  items: T[],
): T[] {
  const selectedByLabel = new Map<string, T>()
  for (const item of items) {
    const label = item.label.trim()
    if (!label) continue
    const selected = selectedByLabel.get(label)
    if (!selected || compareAntigravityQuotaItems(item, selected) < 0) {
      selectedByLabel.set(label, item)
    }
  }
  return Array.from(selectedByLabel.values()).sort(compareAntigravityQuotaItems)
}

function canonicalizeAntigravityQuotaGroupLabel(rawLabel: string): string {
  const normalized = rawLabel
    .trim()
    .toLowerCase()
    .replace(/[._-]+/g, ' ')
    .replace(/\s+/g, ' ')

  if (normalized === 'gemini额度' || /^gemini(?: models?)?$/.test(normalized)) {
    return 'Gemini Models'
  }
  if (
    normalized === 'claude & chatgpt'
    || /^claude\s*(?:&|and)\s*(?:gpt|chatgpt)(?: models?)?$/.test(normalized)
  ) {
    return 'Claude and GPT models'
  }
  return rawLabel.trim()
}

function getAntigravityQuotaGroupName(window: QuotaWindowSnapshot): string {
  const explicitGroupLabel = String(window.quota_group_label || '').trim()
  if (explicitGroupLabel) {
    return canonicalizeAntigravityQuotaGroupLabel(explicitGroupLabel)
  }

  const rawLabel = String(window.label || '').trim()
  const groupLabel = rawLabel.split(/\s*·\s*/, 1)[0]
    ?.replace(/\s+(?:weekly limit remaining|weekly|5\s*hours?|5h)$/i, '')
    .trim()
  return canonicalizeAntigravityQuotaGroupLabel(groupLabel || rawLabel)
}

function getAntigravityQuotaPeriodLabel(
  window: QuotaWindowSnapshot,
  translate: AntigravityQuotaTranslator,
): string {
  const periodCandidates = [window.window, window.bucket_id, window.code, window.label]
    .map(value => String(value || '').trim())
    .filter(Boolean)
  const normalized = periodCandidates.join(' ').toLowerCase()

  if (/(?:^|[^a-z])weekly(?:[^a-z]|$)/.test(normalized)) {
    return translate('providers.antigravity.quota.period.weekly')
  }
  if (/(?:^|[^a-z0-9])5\s*(?:h|hours?)(?:[^a-z0-9]|$)/.test(normalized)) {
    return translate('providers.antigravity.quota.period.fiveHours')
  }

  return String(window.window || '').trim()
}

export function resolveAntigravityQuotaGroupLabel(
  window: QuotaWindowSnapshot,
  translate: AntigravityQuotaTranslator,
): string {
  const groupName = getAntigravityQuotaGroupName(window)
  const periodLabel = getAntigravityQuotaPeriodLabel(window, translate)
  if (groupName && periodLabel) return `${groupName} · ${periodLabel}`
  return groupName || String(window.label || window.code || '').trim()
}
