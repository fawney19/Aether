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
  ultra: { label: 'ultra', description: 'Codex Ultra 预设，请求推理强度为 max' },
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

export function defaultModelDirectiveSuffixesForApiFormat(
  apiFormat: string,
): readonly ModelDirectiveSuffix[] {
  switch (apiFormat) {
    case 'openai:chat':
    case 'openai:responses':
    case 'openai:responses:compact':
    case 'openai:search':
      return MODEL_DIRECTIVE_SUFFIXES
    case 'claude:messages':
    case 'gemini:generate_content':
      return CROSS_PROVIDER_SUFFIXES
    default:
      return []
  }
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
    .map(item => normalizeDirectiveSuffix(item))
    .filter((item): item is string => Boolean(item))
  const next = new Set(normalized)
  if (enabled) next.add(suffix)
  else next.delete(suffix)

  const known = MODEL_DIRECTIVE_SUFFIXES.filter(item => next.delete(item))
  return [...known, ...next]
}

function normalizeMappings(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) {
    return {}
  }

  const normalized: Record<string, unknown> = {}
  for (const [rawSuffix, mapping] of Object.entries(value)) {
    const suffix = normalizeDirectiveSuffix(rawSuffix)
    if (!suffix) continue
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
      .map(item => normalizeDirectiveSuffix(item))
      .filter((item): item is string => Boolean(item)),
  )

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
    for (const [apiFormat, rawConfig] of Object.entries(sourceApiFormats)) {
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
          mappings: normalizeMappings(rawConfig.mappings),
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

function normalizeDirectiveSuffix(value: string): string | undefined {
  const trimmed = value.trim()
  if (!trimmed) return undefined
  const normalized = trimmed.toLowerCase()
  return MODEL_DIRECTIVE_SUFFIXES.some(item => item === normalized)
    ? normalized
    : trimmed
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}
