<template>
  <Dialog
    :model-value="modelValue"
    :title="legacyT('查看规则原值')"
    size="2xl"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="space-y-3">
      <p class="text-sm text-muted-foreground">
        {{ legacyT('仅展示已保存的规则，可能包含密钥；不会覆盖未保存的编辑。关闭后清除明文。') }}
      </p>
      <div
        v-if="loading"
        role="status"
        class="flex items-center gap-2 py-6 text-sm text-muted-foreground"
      >
        <Loader2 class="h-4 w-4 animate-spin" />
        {{ legacyT('加载中...') }}
      </div>
      <p
        v-else-if="failed"
        role="alert"
        class="text-sm text-destructive"
      >
        {{ legacyT('加载规则失败，请关闭后重试') }}
      </p>
      <Textarea
        v-else-if="rulesJson"
        :model-value="rulesJson"
        :aria-label="legacyT('原始规则 JSON')"
        readonly
        spellcheck="false"
        class="min-h-[320px] font-mono text-xs leading-relaxed"
      />
    </div>
    <template #footer>
      <Button
        variant="outline"
        @click="emit('update:modelValue', false)"
      >
        {{ legacyT('关闭') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { Loader2 } from 'lucide-vue-next'
import { Button, Dialog, Textarea } from '@/components/ui'
import { revealEndpointRules } from '@/api/endpoints'
import { useI18n } from '@/i18n'

const props = defineProps<{
  modelValue: boolean
  endpointId: string | null
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()
const { legacyT } = useI18n()
const rulesJson = ref('')
const loading = ref(false)
const failed = ref(false)

watch(() => [props.modelValue, props.endpointId] as const, async ([open, endpointId], _previous, onCleanup) => {
  rulesJson.value = ''
  failed.value = false
  loading.value = false
  if (!open || !endpointId) return

  const controller = new AbortController()
  onCleanup(() => {
    controller.abort()
    rulesJson.value = ''
  })
  loading.value = true
  try {
    const rules = await revealEndpointRules(endpointId, controller.signal)
    if (!controller.signal.aborted) {
      rulesJson.value = JSON.stringify(rules, null, 2)
    }
  } catch {
    if (!controller.signal.aborted) failed.value = true
  } finally {
    if (!controller.signal.aborted) loading.value = false
  }
}, { immediate: true })
</script>
