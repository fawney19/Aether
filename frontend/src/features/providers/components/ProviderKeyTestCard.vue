<template>
  <div
    class="mt-2 rounded-md border border-border/60 bg-muted/20 p-2 space-y-2"
    data-testid="provider-key-test-card"
  >
    <div class="flex items-center justify-between gap-2">
      <span class="text-[10px] text-muted-foreground">测试模型是否可用</span>
      <span
        v-if="statusLabel"
        class="text-[10px]"
        :class="statusClass"
      >
        {{ statusLabel }}
      </span>
    </div>
    <div class="flex items-center gap-1.5">
      <Select
        v-if="modelOptions.length > 0"
        :model-value="modelName"
        :disabled="testing || loadingModels"
        @update:model-value="modelName = $event"
      >
        <SelectTrigger
          class="h-7 min-w-0 flex-1 rounded-md px-2 text-xs"
          data-testid="provider-key-test-model-select"
        >
          <SelectValue :placeholder="loadingModels ? '加载模型…' : '选择测试模型'" />
        </SelectTrigger>
        <SelectContent
          searchable
          :search-threshold="0"
          search-placeholder="搜索模型"
        >
          <SelectItem
            v-for="option in modelOptions"
            :key="option.value"
            :value="option.value"
            :text-value="option.label"
          >
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>
      <Input
        v-else
        v-model="modelName"
        class="h-7 text-xs"
        :placeholder="loadingModels ? '加载模型…' : '模型名，如 gpt-5.4'"
        :disabled="testing || loadingModels"
        data-testid="provider-key-test-model-input"
      />
      <Button
        size="sm"
        class="h-7 px-2 text-[11px] shrink-0"
        :disabled="testing || loadingModels || !modelName.trim()"
        data-testid="provider-key-test-run"
        @click="runTest"
      >
        {{ testing ? '测试中' : '测试' }}
      </Button>
    </div>
    <pre
      v-if="output"
      class="max-h-24 overflow-auto rounded bg-background/80 p-2 font-mono text-[10px] leading-4 text-muted-foreground whitespace-pre-wrap"
    >{{ output }}</pre>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Select from '@/components/ui/select.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import { useModelTest } from '@/composables/useModelTest'
import { getProviderModels } from '@/api/endpoints/models'
import type { EndpointAPIKey } from '@/api/endpoints/types'
import type { Model } from '@/api/endpoints/types/model'

const props = withDefaults(defineProps<{
  providerId: string
  apiKey: EndpointAPIKey
  models?: Model[]
  loadingModels?: boolean
}>(), {
  models: () => [],
  loadingModels: false,
})

const modelName = ref('')
const output = ref('')
const loadedModels = ref<Model[]>([])
const loadingRemoteModels = ref(false)
const modelTest = useModelTest({ providerId: () => props.providerId })
const testing = computed(() => modelTest.testing.value)

const resolvedModels = computed(() => (
  props.models.length > 0 ? props.models : loadedModels.value
))

const modelOptions = computed(() => {
  const seen = new Set<string>()
  return resolvedModels.value
    .filter((model) => model.is_active !== false)
    .map((model) => {
      const value = (model.provider_model_name || model.global_model_name || '').trim()
      const label = (
        model.global_model_display_name
        || model.global_model_name
        || model.provider_model_name
        || value
      ).trim()
      return { value, label: label && label !== value ? `${label} (${value})` : label || value }
    })
    .filter((option) => {
      if (!option.value || seen.has(option.value)) return false
      seen.add(option.value)
      return true
    })
})

const statusLabel = computed(() => {
  const result = modelTest.testResult.value
  if (testing.value) return '连接中…'
  if (!result) return ''
  return result.success ? '可用' : '失败'
})

const statusClass = computed(() => {
  const result = modelTest.testResult.value
  if (testing.value) return 'text-amber-600'
  if (!result) return ''
  return result.success ? 'text-emerald-600' : 'text-destructive'
})

function pickDefaultModel(options: Array<{ value: string }>): string {
  const preferred = [
    'gpt-5.4',
    'gpt-5',
    'gpt-5.4-codex',
    'gpt-4.1',
    'codex',
  ]
  for (const name of preferred) {
    const match = options.find((option) => option.value.toLowerCase() === name)
    if (match) return match.value
  }
  const fuzzy = options.find((option) => /gpt-5|codex|sonnet/i.test(option.value))
  return fuzzy?.value || options[0]?.value || ''
}

watch(modelOptions, (options) => {
  if (!modelName.value && options.length > 0) {
    modelName.value = pickDefaultModel(options)
  }
}, { immediate: true })

watch(() => props.providerId, async (providerId, previousProviderId) => {
  if (previousProviderId !== undefined) {
    modelName.value = ''
    loadedModels.value = []
  }
  if (!providerId || props.models.length > 0) return
  loadingRemoteModels.value = true
  try {
    loadedModels.value = await getProviderModels(providerId, { is_active: true, limit: 1000 })
  } catch {
    loadedModels.value = []
  } finally {
    loadingRemoteModels.value = false
  }
}, { immediate: true })

const loadingModels = computed(() => props.loadingModels || loadingRemoteModels.value)

async function runTest() {
  const name = modelName.value.trim()
  if (!name) return
  output.value = `测试 ${name} …`
  await modelTest.startTest({
    mode: 'direct',
    modelName: name,
    displayLabel: name,
    apiKeyIds: [props.apiKey.id],
    applyModelMapping: true,
    onSuccess: (result) => {
      const preview = typeof result.data === 'object' && result.data
        ? JSON.stringify(result.data).slice(0, 400)
        : ''
      output.value = preview ? `成功\n${preview}` : '成功'
    },
    onFailure: (result) => {
      output.value = result.error?.trim() || '测试失败'
      return true
    },
    onError: (err) => {
      output.value = err instanceof Error ? err.message : '测试失败'
      return true
    },
  })
}
</script>
