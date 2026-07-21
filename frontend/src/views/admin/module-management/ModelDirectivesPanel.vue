<template>
  <div class="space-y-4">
    <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div>
        <h3 class="text-base font-semibold">
          模型参数指令
        </h3>
        <p class="mt-1 text-sm text-muted-foreground">
          各端点可分别启用正式参数映射，并在必要时覆盖内置值。
        </p>
      </div>
      <div class="flex items-center">
        <Switch
          :model-value="config.reasoning_effort.enabled"
          :disabled="loading"
          aria-label="启用模型参数指令"
          @update:model-value="onReasoningEnabledChange"
        />
      </div>
    </div>

    <div class="overflow-hidden rounded-lg border">
      <div class="hidden gap-2 border-b bg-muted/40 px-4 py-3 text-xs font-medium text-muted-foreground lg:grid lg:grid-cols-[minmax(0,1fr)_minmax(0,0.7fr)_minmax(0,1.8fr)_auto]">
        <div>API 端点</div>
        <div>模型指令</div>
        <div>自定义映射</div>
        <div class="text-right">
          状态
        </div>
      </div>
      <div class="divide-y">
        <div
          v-for="format in MODEL_DIRECTIVE_API_FORMATS"
          :key="format.key"
          class="grid grid-cols-1 items-center gap-3 px-4 py-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,0.7fr)_minmax(0,1.8fr)_auto]"
        >
          <div>
            <div class="mb-1 text-xs font-medium text-muted-foreground lg:hidden">
              API 端点
            </div>
            <div class="text-sm font-medium">
              {{ format.label }}
            </div>
            <code class="mt-1 block text-xs text-muted-foreground">
              {{ format.parameter }}
            </code>
          </div>
          <div>
            <div class="mb-1 text-xs font-medium text-muted-foreground lg:hidden">
              模型指令
            </div>
            <Select
              :model-value="selectedSuffixes[format.key] ?? 'low'"
              :disabled="loading || !config.reasoning_effort.enabled || !formatConfig(format.key).enabled"
              @update:model-value="value => selectSuffix(format.key, value)"
            >
              <SelectTrigger
                class="h-9 w-full rounded-lg lg:w-32"
                :aria-label="`${format.label} 模型指令`"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="suffix in availableSuffixes(format.key)"
                  :key="suffix"
                  :value="suffix"
                  :text-value="suffixLabel(suffix)"
                >
                  {{ suffixLabel(suffix) }}
                </SelectItem>
              </SelectContent>
            </Select>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ selectedSuffixDescription(format.key) }}
            </p>
            <div class="mt-2 flex items-center justify-between gap-2">
              <span class="text-xs text-muted-foreground">启用此指令</span>
              <Switch
                :model-value="selectedSuffixEnabled(format.key)"
                :disabled="loading || !config.reasoning_effort.enabled || !formatConfig(format.key).enabled"
                :aria-label="`${format.label} ${selectedSuffixes[format.key] ?? 'low'} 指令`"
                @update:model-value="value => onSuffixEnabledChange(format.key, value)"
              />
            </div>
          </div>
          <div>
            <div class="mb-1 text-xs font-medium text-muted-foreground lg:hidden">
              自定义映射
            </div>
            <div class="flex items-start gap-2">
              <div class="min-w-0 flex-1">
                <Textarea
                  :id="mappingInputId(format.key)"
                  :model-value="localMappingParams[mappingKey(format.key)]"
                  class="h-24 min-h-24 resize-none overflow-auto font-mono text-xs leading-5"
                  :disabled="loading || !config.reasoning_effort.enabled || !formatConfig(format.key).enabled || !selectedSuffixEnabled(format.key)"
                  :aria-label="`${format.label} ${selectedSuffixes[format.key] ?? 'low'} 映射参数`"
                  :aria-invalid="Boolean(mappingErrors[mappingKey(format.key)])"
                  :aria-describedby="mappingErrors[mappingKey(format.key)] ? mappingErrorId(format.key) : undefined"
                  title="自定义映射 JSON"
                  placeholder="{}"
                  @update:model-value="value => onMappingDraftChange(format.key, value)"
                />
                <p
                  v-if="mappingErrors[mappingKey(format.key)]"
                  :id="mappingErrorId(format.key)"
                  class="mt-1 text-xs text-destructive"
                  role="alert"
                >
                  {{ mappingErrors[mappingKey(format.key)] }}
                </p>
                <p
                  v-else
                  class="mt-1 text-xs text-muted-foreground"
                >
                  {{ mappingStatus(format.key) }}
                </p>
              </div>
              <Button
                size="icon"
                variant="ghost"
                class="h-9 w-9 shrink-0 text-muted-foreground"
                :disabled="loading || !config.reasoning_effort.enabled || !formatConfig(format.key).enabled || !selectedSuffixEnabled(format.key) || !hasMappingParamChanges(format.key)"
                :title="`保存 ${format.label} 映射参数`"
                :aria-label="`保存 ${format.label} 映射参数`"
                @click="saveMappingParam(format.key)"
              >
                <Save class="h-4 w-4" />
              </Button>
            </div>
          </div>
          <div class="flex items-center justify-between gap-3 lg:justify-end">
            <span class="text-xs font-medium text-muted-foreground lg:hidden">状态</span>
            <Switch
              :model-value="formatConfig(format.key).enabled"
              :disabled="loading || !config.reasoning_effort.enabled"
              :aria-label="`${format.label} 模型参数指令`"
              @update:model-value="value => onApiFormatEnabledChange(format.key, value)"
            />
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { reactive, watch } from 'vue'
import { Save } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import Switch from '@/components/ui/switch.vue'
import Textarea from '@/components/ui/textarea.vue'
import {
  MODEL_DIRECTIVE_SUFFIX_METADATA,
  MODEL_DIRECTIVE_API_FORMATS,
  defaultModelDirectiveSuffixesForApiFormat,
  updateModelDirectiveMappingOverride,
  updateModelDirectiveSuffixEnabled,
  type ReasoningApiFormatConfig,
  type ModelDirectivesConfig,
} from './modelDirectivesConfig'

const props = defineProps<{
  config: ModelDirectivesConfig
  loading: boolean
}>()

const emit = defineEmits<{
  save: [value: ModelDirectivesConfig]
}>()

const selectedSuffixes = reactive<Record<string, string>>({})
const localMappingParams = reactive<Record<string, string>>({})
const mappingErrors = reactive<Record<string, string>>({})
const dirtyMappingKeys = reactive(new Set<string>())

watch(() => props.config.reasoning_effort.api_formats, (newFormats) => {
  for (const format of MODEL_DIRECTIVE_API_FORMATS) {
    const fc = newFormats[format.key]
    const selectedSuffix = selectedSuffixes[format.key]
      ?? firstConfiguredSuffix(format.key, fc)
      ?? 'low'
    selectedSuffixes[format.key] = selectedSuffix
    for (const suffix of availableSuffixes(format.key)) {
      const key = mappingKey(format.key, suffix)
      const authoritativeText = mappingOverrideText(fc?.mappings?.[suffix])
      if (dirtyMappingKeys.has(key)) {
        if (localMappingParams[key] === authoritativeText) {
          dirtyMappingKeys.delete(key)
          delete mappingErrors[key]
        }
        continue
      }
      localMappingParams[key] = authoritativeText
      delete mappingErrors[key]
    }
  }
}, { immediate: true })

function formatConfig(apiFormat: string): ReasoningApiFormatConfig {
  return props.config.reasoning_effort.api_formats[apiFormat] ?? {
    enabled: true,
    suffixes: [...defaultModelDirectiveSuffixesForApiFormat(apiFormat)],
    mappings: {},
  }
}

function availableSuffixes(apiFormat: string): string[] {
  const configured = props.config.reasoning_effort.api_formats[apiFormat]
  return [...new Set([
    ...defaultModelDirectiveSuffixesForApiFormat(apiFormat),
    ...(configured?.suffixes ?? []),
    ...Object.keys(configured?.mappings ?? {}),
  ])]
}

function firstConfiguredSuffix(
  apiFormat: string,
  config: ReasoningApiFormatConfig | undefined,
): string | undefined {
  return availableSuffixes(apiFormat).find(suffix => (
    config?.suffixes.includes(suffix) || config?.mappings?.[suffix] !== undefined
  ))
}

function mappingKey(
  apiFormat: string,
  suffix: string = selectedSuffixes[apiFormat] ?? 'low',
): string {
  return `${apiFormat}:${suffix}`
}

function selectSuffix(apiFormat: string, suffix: string) {
  if (!availableSuffixes(apiFormat).includes(suffix)) {
    throw new Error(`Unsupported model directive suffix: ${suffix}`)
  }
  selectedSuffixes[apiFormat] = suffix
  const key = mappingKey(apiFormat, suffix)
  if (!Object.prototype.hasOwnProperty.call(localMappingParams, key)) {
    localMappingParams[key] = mappingOverrideText(formatConfig(apiFormat).mappings[suffix])
    delete mappingErrors[key]
  }
}

function onMappingDraftChange(apiFormat: string, value: string) {
  const suffix = selectedSuffixes[apiFormat] ?? 'low'
  const key = mappingKey(apiFormat, suffix)
  localMappingParams[key] = value
  if (value === mappingOverrideText(formatConfig(apiFormat).mappings[suffix])) {
    dirtyMappingKeys.delete(key)
  } else {
    dirtyMappingKeys.add(key)
  }
  delete mappingErrors[key]
}

function suffixLabel(suffix: string): string {
  return MODEL_DIRECTIVE_SUFFIX_METADATA[suffix as keyof typeof MODEL_DIRECTIVE_SUFFIX_METADATA]?.label
    ?? suffix
}

function selectedSuffixDescription(apiFormat: string): string {
  const suffix = selectedSuffixes[apiFormat] ?? 'low'
  return MODEL_DIRECTIVE_SUFFIX_METADATA[suffix as keyof typeof MODEL_DIRECTIVE_SUFFIX_METADATA]?.description
    ?? '自定义模型指令'
}

function selectedSuffixEnabled(apiFormat: string): boolean {
  return formatConfig(apiFormat).suffixes.includes(selectedSuffixes[apiFormat] ?? 'low')
}

function mappingInputId(apiFormat: string): string {
  return `model-directive-mapping-${apiFormat.replace(/[^a-z0-9]+/gi, '-')}`
}

function mappingErrorId(apiFormat: string): string {
  return `${mappingInputId(apiFormat)}-error`
}

function mappingOverrideText(mapping: unknown): string {
  return mapping === undefined ? '' : JSON.stringify(mapping, null, 2)
}

function hasCustomMapping(apiFormat: string): boolean {
  const suffix = selectedSuffixes[apiFormat] ?? 'low'
  return Object.prototype.hasOwnProperty.call(formatConfig(apiFormat).mappings, suffix)
}

function mappingStatus(apiFormat: string): string {
  return hasCustomMapping(apiFormat)
    ? '自定义映射'
    : '内置映射'
}

function hasMappingParamChanges(apiFormat: string): boolean {
  const suffix = selectedSuffixes[apiFormat] ?? 'low'
  return (localMappingParams[mappingKey(apiFormat, suffix)] ?? '')
    !== mappingOverrideText(formatConfig(apiFormat).mappings[suffix])
}

function onReasoningEnabledChange(value: boolean) {
  emit('save', {
    ...props.config,
    reasoning_effort: { ...props.config.reasoning_effort, enabled: Boolean(value) },
  })
}

function onApiFormatEnabledChange(apiFormat: string, value: boolean) {
  const current = formatConfig(apiFormat)
  emit('save', {
    ...props.config,
    reasoning_effort: {
      ...props.config.reasoning_effort,
      api_formats: {
        ...props.config.reasoning_effort.api_formats,
        [apiFormat]: { ...current, enabled: Boolean(value) },
      },
    },
  })
}

function onSuffixEnabledChange(apiFormat: string, value: boolean) {
  const current = formatConfig(apiFormat)
  const suffix = selectedSuffixes[apiFormat] ?? 'low'
  emit('save', {
    ...props.config,
    reasoning_effort: {
      ...props.config.reasoning_effort,
      api_formats: {
        ...props.config.reasoning_effort.api_formats,
        [apiFormat]: {
          ...current,
          suffixes: updateModelDirectiveSuffixEnabled(current.suffixes, suffix, Boolean(value)),
        },
      },
    },
  })
}

function saveMappingParam(apiFormat: string) {
  const current = formatConfig(apiFormat)
  const suffix = selectedSuffixes[apiFormat] ?? 'low'
  const key = mappingKey(apiFormat, suffix)
  const rawMapping = (localMappingParams[key] ?? '').trim()
  let mapping: Record<string, unknown> | undefined
  if (rawMapping) {
    try {
      const parsed = JSON.parse(rawMapping)
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        mappingErrors[key] = '映射参数必须是 JSON 对象'
        return
      }
      mapping = Object.keys(parsed).length > 0
        ? parsed as Record<string, unknown>
        : undefined
    } catch {
      mappingErrors[key] = 'JSON 格式无效，请修正后再保存'
      return
    }
  }

  delete mappingErrors[key]
  localMappingParams[key] = mappingOverrideText(mapping)
  const mappings = updateModelDirectiveMappingOverride(current.mappings, suffix, mapping)
  emit('save', {
    ...props.config,
    reasoning_effort: {
      ...props.config.reasoning_effort,
      api_formats: {
        ...props.config.reasoning_effort.api_formats,
        [apiFormat]: {
          ...current,
          mappings,
        },
      },
    },
  })
}
</script>
