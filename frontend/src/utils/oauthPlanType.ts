const PLAN_TYPE_LABELS: Record<string, string> = {
  free: 'Free',
  plus: 'Plus',
  team: 'Team',
  enterprise: 'Enterprise',
  paid: 'Paid',
  pro: 'Pro',
  'pro+': 'Pro+',
  power: 'Power',
  ultra: 'Ultra',
}

const PLAN_TYPE_CLASS_NAMES: Record<string, string> = {
  plus: 'border-green-500/50 text-green-600 dark:text-green-400',
  pro: 'border-blue-500/50 text-blue-600 dark:text-blue-400',
  free: 'border-primary/50 text-primary',
  paid: 'border-blue-500/50 text-blue-600 dark:text-blue-400',
  team: 'border-purple-500/50 text-purple-600 dark:text-purple-400',
  enterprise: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
  ultra: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
  'pro+': 'border-purple-500/50 text-purple-600 dark:text-purple-400',
  power: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
}

export function normalizeOAuthPlanType(planType?: string | null): string | null {
  if (typeof planType !== 'string') {
    return null
  }

  const normalized = planType.trim().toLowerCase()
  if (!normalized) {
    return null
  }
  return normalized
}

export function formatOAuthPlanType(planType?: string | null): string {
  const normalized = normalizeOAuthPlanType(planType)
  if (!normalized) {
    return ''
  }

  const knownLabel = PLAN_TYPE_LABELS[normalized]
  if (knownLabel) {
    return knownLabel
  }

  return normalized
    .replace(/[_-]+/g, ' ')
    .split(/\s+/)
    .filter(Boolean)
    .map(part => part[0].toUpperCase() + part.slice(1))
    .join(' ')
}

export function getOAuthPlanTypeClass(planType?: string | null): string {
  const normalized = normalizeOAuthPlanType(planType)
  if (!normalized) {
    return ''
  }
  return PLAN_TYPE_CLASS_NAMES[normalized] || ''
}

export function isNonFreeOAuthPlan(planType?: string | null): boolean {
  const normalized = normalizeOAuthPlanType(planType)
  if (!normalized) {
    return false
  }
  return normalized !== 'free'
}
