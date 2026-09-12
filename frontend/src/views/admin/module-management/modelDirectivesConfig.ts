import { normalizeApiFormatAlias } from '@/api/endpoints/types/api-format'

export interface ReasoningApiFormatConfig {
  [key: string]: unknown
  enabled: boolean
  suffixes: string[]
  mappings: Record<string, unknown>
}

export interface ReasoningEffortConfig {
  [key: string]: unknown
  enabled: boolean
  api_formats: Record<string, ReasoningApiFormatConfig>
}

export interface ModelDirectivesConfig {
  [key: string]: unknown
  reasoning_effort: ReasoningEffortConfig
}

export const MODEL_DIRECTIVES_MODULE_NAME = 'model_directives'

export const REASONING_EFFORTS = [
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
] as const

export type ReasoningEffort = typeof REASONING_EFFORTS[number]

export const MODEL_DIRECTIVE_SUFFIXES = [...REASONING_EFFORTS, 'ultra', 'fast'] as const
export type ModelDirectiveSuffix = typeof MODEL_DIRECTIVE_SUFFIXES[number]

export const PREFERRED_MODEL_DIRECTIVE_SUFFIX: ModelDirectiveSuffix = 'low'

export const MODEL_DIRECTIVE_SUFFIX_METADATA: Readonly<
  Record<ModelDirectiveSuffix, { label: string, description: string }>
> = {
  none: { label: 'none', description: '不启用推理' },
  minimal: { label: 'minimal', description: '模型支持时使用最低推理投入' },
  low: { label: 'low', description: '低推理投入' },
  medium: { label: 'medium', description: '中等推理投入' },
  high: { label: 'high', description: '高推理投入' },
  xhigh: { label: 'xhigh', description: '超高推理投入' },
  max: { label: 'max', description: '模型支持时使用最大推理投入' },
  ultra: { label: 'ultra', description: 'Codex Ultra 推理预设，仅支持兼容的 Codex 模型' },
  fast: { label: 'fast', description: 'Fast 服务层级' },
}

export const MODEL_DIRECTIVE_API_FORMATS = [
  {
    key: 'openai:chat',
    label: 'OpenAI Chat',
    parameter: 'reasoning_effort',
  },
  {
    key: 'openai:responses',
    label: 'OpenAI Responses',
    parameter: 'reasoning.effort',
  },
  {
    key: 'openai:responses:compact',
    label: 'OpenAI Responses Compact',
    parameter: 'reasoning.effort',
  },
  {
    key: 'openai:search',
    label: 'OpenAI Search',
    parameter: 'reasoning.effort',
  },
  {
    key: 'claude:messages',
    label: 'Claude Messages',
    parameter: 'output_config.effort + thinking',
  },
  {
    key: 'gemini:generate_content',
    label: 'Gemini GenerateContent',
    parameter: 'generationConfig.thinkingConfig',
  },
] as const

const CROSS_PROVIDER_SUFFIXES: readonly ModelDirectiveSuffix[] = [
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
]

const OPENAI_SEARCH_SUFFIXES: readonly ModelDirectiveSuffix[] = MODEL_DIRECTIVE_SUFFIXES
  .filter(suffix => suffix !== 'fast')

export function defaultModelDirectiveSuffixesForApiFormat(
  apiFormat: string,
): readonly ModelDirectiveSuffix[] {
  switch (normalizeModelDirectiveApiFormat(apiFormat)) {
    case 'openai:chat':
    case 'openai:responses':
    case 'openai:responses:compact':
      return MODEL_DIRECTIVE_SUFFIXES
    case 'openai:search':
      return OPENAI_SEARCH_SUFFIXES
    case 'claude:messages':
    case 'gemini:generate_content':
      return CROSS_PROVIDER_SUFFIXES
    default:
      return []
  }
}

/**
 * Returns the generic built-in patch shown by the admin UI. The runtime may
 * refine Claude/Gemini shapes and model capability checks for the mapped model.
 */
export function modelDirectiveBuiltInMappingPreview(
  apiFormat: string,
  suffix: string,
): Record<string, unknown> | undefined {
  apiFormat = normalizeModelDirectiveApiFormat(apiFormat)
  const normalizedSuffix = normalizeModelDirectiveSuffix(suffix)
  if (!normalizedSuffix) return undefined
  if (!modelDirectiveSuffixSupportedForApiFormat(apiFormat, normalizedSuffix)) return undefined

  if (normalizedSuffix === 'fast') {
    if (apiFormat === 'openai:search') return undefined
    return ['openai:chat', 'openai:responses', 'openai:responses:compact'].includes(apiFormat)
      ? { service_tier: 'priority' }
      : undefined
  }

  const reasoningEffort = REASONING_EFFORTS.find(effort => effort === normalizedSuffix)
  const isCodexUltra = normalizedSuffix === 'ultra'
  if (!reasoningEffort && !isCodexUltra) return undefined

  switch (apiFormat) {
    case 'openai:chat':
      return { reasoning_effort: normalizedSuffix }
    case 'openai:responses':
    case 'openai:responses:compact':
    case 'openai:search':
      return { reasoning: { effort: normalizedSuffix } }
    case 'claude:messages': {
      if (!reasoningEffort) return undefined
      return {
        output_config: { effort: claudeEffortValue(reasoningEffort) },
        thinking: {
          type: 'enabled',
          budget_tokens: thinkingBudgetTokens(reasoningEffort),
        },
      }
    }
    case 'gemini:generate_content': {
      if (!reasoningEffort) return undefined
      return {
        generationConfig: {
          thinkingConfig: {
            includeThoughts: true,
            thinkingBudget: thinkingBudgetTokens(reasoningEffort),
          },
        },
      }
    }
    default:
      return undefined
  }
}

export function modelDirectiveEffectiveMappingPreview(
  apiFormat: string,
  suffix: string,
  override: unknown,
): unknown {
  const builtIn = modelDirectiveBuiltInMappingPreview(apiFormat, suffix)
  if (override === undefined) return cloneJsonValue(builtIn)
  if (builtIn === undefined) return cloneJsonValue(override)
  return deepMergeJsonValue(builtIn, override)
}

export function modelDirectiveMappingOverrideFromEffective(
  apiFormat: string,
  suffix: string,
  effectiveMapping: Record<string, unknown>,
): Record<string, unknown> | undefined {
  const builtIn = modelDirectiveBuiltInMappingPreview(apiFormat, suffix)
  if (!builtIn) return Object.keys(effectiveMapping).length > 0
    ? cloneJsonValue(effectiveMapping)
    : undefined

  const override = jsonValueDifference(builtIn, effectiveMapping)
  return isRecord(override) && Object.keys(override).length > 0
    ? override
    : undefined
}

function claudeEffortValue(effort: ReasoningEffort): string {
  return effort === 'none' || effort === 'minimal' ? 'low' : effort
}

function thinkingBudgetTokens(effort: ReasoningEffort): number {
  switch (effort) {
    case 'none':
      return 0
    case 'minimal':
      return 512
    case 'low':
      return 1280
    case 'medium':
      return 2048
    case 'high':
      return 4096
    case 'xhigh':
    case 'max':
      return 8192
  }
}

function deepMergeJsonValue(target: unknown, patch: unknown): unknown {
  if (!isRecord(target) || !isRecord(patch)) return cloneJsonValue(patch)

  const merged: Record<string, unknown> = cloneJsonValue(target)
  for (const [key, patchValue] of Object.entries(patch)) {
    merged[key] = Object.prototype.hasOwnProperty.call(merged, key)
      ? deepMergeJsonValue(merged[key], patchValue)
      : cloneJsonValue(patchValue)
  }
  return merged
}

function jsonValueDifference(base: unknown, value: unknown): unknown {
  if (isRecord(base) && isRecord(value)) {
    const difference: Record<string, unknown> = {}
    for (const [key, item] of Object.entries(value)) {
      if (!Object.prototype.hasOwnProperty.call(base, key)) {
        difference[key] = cloneJsonValue(item)
        continue
      }
      const nestedDifference = jsonValueDifference(base[key], item)
      if (nestedDifference !== undefined) difference[key] = nestedDifference
    }
    return Object.keys(difference).length > 0 ? difference : undefined
  }

  return JSON.stringify(base) === JSON.stringify(value)
    ? undefined
    : cloneJsonValue(value)
}

function cloneJsonValue<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map(item => cloneJsonValue(item)) as T
  }
  if (isRecord(value)) {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, cloneJsonValue(item)]),
    ) as T
  }
  return value
}

export function createDefaultModelDirectivesConfig(): ModelDirectivesConfig {
  return {
    reasoning_effort: {
      enabled: true,
      api_formats: Object.fromEntries(
        MODEL_DIRECTIVE_API_FORMATS.map((format) => [
          format.key,
          {
            enabled: true,
            suffixes: [...defaultModelDirectiveSuffixesForApiFormat(format.key)],
            mappings: {},
          },
        ])
      ),
    },
  }
}

export function updateModelDirectiveMappingOverride(
  mappings: Record<string, unknown>,
  suffix: string,
  override: Record<string, unknown> | undefined,
): Record<string, unknown> {
  const nextMappings = { ...mappings }
  if (override === undefined || Object.keys(override).length === 0) {
    delete nextMappings[suffix]
  } else {
    nextMappings[suffix] = override
  }
  return nextMappings
}

export function updateModelDirectiveSuffixEnabled(
  suffixes: string[],
  suffix: string,
  enabled: boolean,
): string[] {
  const normalized = suffixes
    .map(item => normalizeModelDirectiveSuffix(item))
    .filter((item): item is string => Boolean(item))
  const next = new Set(normalized)
  if (enabled) next.add(suffix)
  else next.delete(suffix)

  const known = MODEL_DIRECTIVE_SUFFIXES.filter(item => next.delete(item))
  return [...known, ...next]
}

function normalizeMappings(apiFormat: string, value: unknown): Record<string, unknown> {
  if (!isRecord(value)) {
    return {}
  }

  const normalized: Record<string, unknown> = {}
  for (const [rawSuffix, mapping] of Object.entries(value)) {
    const suffix = normalizeModelDirectiveSuffix(rawSuffix)
    if (!suffix) continue
    if (!modelDirectiveSuffixSupportedForApiFormat(apiFormat, suffix)) continue
    normalized[suffix] = mapping
  }
  return normalized
}

function normalizeSuffixes(
  apiFormat: string,
  value: unknown,
  configuredMappings: unknown,
): string[] {
  const source = Array.isArray(value)
    ? value
    : isRecord(configuredMappings)
      ? Object.keys(configuredMappings)
      : defaultModelDirectiveSuffixesForApiFormat(apiFormat)
  const normalized = new Set(
    source
      .filter((item): item is string => typeof item === 'string')
      .map(item => normalizeModelDirectiveSuffix(item))
      .filter((item): item is string => Boolean(item)),
  )
  for (const suffix of normalized) {
    if (!modelDirectiveSuffixSupportedForApiFormat(apiFormat, suffix)) {
      normalized.delete(suffix)
    }
  }

  const known = MODEL_DIRECTIVE_SUFFIXES.filter(item => normalized.delete(item))
  return [...known, ...normalized]
}

export function normalizeModelDirectivesConfig(value: unknown): ModelDirectivesConfig {
  const defaults = createDefaultModelDirectivesConfig()
  if (!isRecord(value)) return defaults

  const source = value
  const reasoning = isRecord(source.reasoning_effort) ? source.reasoning_effort : {}
  const apiFormats: Record<string, ReasoningApiFormatConfig> = {
    ...defaults.reasoning_effort.api_formats,
  }
  const sourceApiFormats = reasoning.api_formats
  if (isRecord(sourceApiFormats)) {
    const sourceEntries = Object.entries(sourceApiFormats).sort(([left], [right]) => {
      const leftCanonical = normalizeModelDirectiveApiFormat(left)
      const rightCanonical = normalizeModelDirectiveApiFormat(right)
      const leftIsCanonical = left === leftCanonical
      const rightIsCanonical = right === rightCanonical
      return Number(leftIsCanonical) - Number(rightIsCanonical)
    })
    for (const [rawApiFormat, rawConfig] of sourceEntries) {
      const apiFormat = normalizeModelDirectiveApiFormat(rawApiFormat)
      if (typeof rawConfig === 'boolean') {
        apiFormats[apiFormat] = {
          enabled: rawConfig,
          suffixes: [...defaultModelDirectiveSuffixesForApiFormat(apiFormat)],
          mappings: {},
        }
      } else if (isRecord(rawConfig)) {
        const preservedConfig = { ...rawConfig }
        apiFormats[apiFormat] = {
          ...preservedConfig,
          enabled: typeof rawConfig.enabled === 'boolean' ? rawConfig.enabled : true,
          suffixes: normalizeSuffixes(apiFormat, rawConfig.suffixes, rawConfig.mappings),
          mappings: normalizeMappings(apiFormat, rawConfig.mappings),
        }
      }
    }
  }

  return {
    ...source,
    reasoning_effort: {
      ...reasoning,
      enabled:
        typeof reasoning.enabled === 'boolean'
          ? reasoning.enabled
          : defaults.reasoning_effort.enabled,
      api_formats: apiFormats,
    },
  }
}

export function normalizeModelDirectiveSuffix(value: string): string | undefined {
  const trimmed = value.trim()
  if (!trimmed) return undefined
  const normalized = trimmed.toLowerCase()
  return MODEL_DIRECTIVE_SUFFIXES.some(item => item === normalized)
    ? normalized
    : trimmed
}

function normalizeModelDirectiveApiFormat(value: string): string {
  const normalized = normalizeApiFormatAlias(value)
  switch (normalized) {
    case 'openai:cli':
      return 'openai:responses'
    case 'openai:compact':
      return 'openai:responses:compact'
    case 'claude:chat':
    case 'claude:cli':
      return 'claude:messages'
    case 'gemini:chat':
    case 'gemini:cli':
      return 'gemini:generate_content'
    case '/v1/chat/completions':
      return 'openai:chat'
    case '/v1/responses':
      return 'openai:responses'
    case '/v1/responses/compact':
      return 'openai:responses:compact'
    case '/v1/alpha/search':
      return 'openai:search'
    case '/v1/messages':
      return 'claude:messages'
    default:
      return normalized
  }
}

function modelDirectiveSuffixSupportedForApiFormat(
  apiFormat: string,
  suffix: string,
): boolean {
  const normalizedSuffix = normalizeModelDirectiveSuffix(suffix)
  if (!normalizedSuffix) return false
  const builtInSuffix = MODEL_DIRECTIVE_SUFFIXES.find(item => item === normalizedSuffix)
  return !builtInSuffix
    || defaultModelDirectiveSuffixesForApiFormat(apiFormat).includes(builtInSuffix)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}
