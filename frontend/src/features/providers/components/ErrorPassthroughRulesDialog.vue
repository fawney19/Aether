<template>
  <Dialog
    :model-value="open"
    title="错误透传规则"
    description="配置哪些上游错误可以直接返回给客户端。仅在状态码和正则同时命中时透传原始错误信息。"
    :icon="AlertTriangle"
    size="lg"
    @update:model-value="handleClose"
  >
    <div class="space-y-3 max-h-[60vh] overflow-y-auto px-0.5 py-0.5 -mx-0.5">
      <div class="flex items-start justify-between gap-3">
        <div class="min-w-0">
          <h3 class="text-sm font-medium">
            透传规则
          </h3>
          <p class="text-xs text-muted-foreground mt-0.5">
            命中后会把上游错误消息直接返回给客户端；未命中仍返回系统兜底文案
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          class="shrink-0"
          @click="addRule"
        >
          <Plus class="w-4 h-4 mr-1" />
          添加
        </Button>
      </div>

      <div
        v-if="rules.length === 0"
        class="text-xs text-muted-foreground px-3 py-4 border border-dashed rounded-lg text-center"
      >
        暂无规则
      </div>

      <div
        v-for="(rule, index) in rules"
        :key="index"
        class="flex items-center gap-1"
      >
        <Input
          v-model="statusCodeInputs[index]"
          placeholder="状态码 (可选)"
          size="sm"
          class="font-mono text-xs w-28 shrink-0"
        />
        <Input
          v-model="rule.pattern"
          placeholder="例如: content_policy_violation"
          size="sm"
          class="font-mono text-xs flex-1"
        />
        <Button
          variant="ghost"
          size="sm"
          class="shrink-0 h-8 w-8 p-0 text-muted-foreground hover:text-destructive"
          @click="removeRule(index)"
        >
          <Trash2 class="w-3.5 h-3.5" />
        </Button>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="saving"
        @click="handleClose"
      >
        取消
      </Button>
      <Button
        :disabled="saving"
        @click="handleSave"
      >
        {{ saving ? '保存中...' : '保存' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import {
  Dialog,
  Button,
  Input,
} from '@/components/ui'
import { AlertTriangle, Plus, Trash2 } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { updateProvider, type ProviderWithEndpointsSummary } from '@/api/endpoints'
import { parseApiError } from '@/utils/errorParser'
import type { FailoverRuleItem } from '@/api/endpoints/types'

const props = defineProps<{
  open: boolean
  provider: ProviderWithEndpointsSummary | null
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'saved': []
}>()

const { success, error: showError } = useToast()
const saving = ref(false)
const rules = ref<FailoverRuleItem[]>([])
const statusCodeInputs = ref<string[]>([])

watch(() => [props.open, props.provider], () => {
  if (props.open && props.provider) {
    const currentRules = props.provider.error_passthrough_rules?.patterns || []
    rules.value = currentRules.map(rule => ({ ...rule }))
    statusCodeInputs.value = rules.value.map(rule =>
      rule.status_codes?.length ? rule.status_codes.join(',') : '',
    )
  }
}, { immediate: true })

function addRule() {
  rules.value.push({ pattern: '', description: '' })
  statusCodeInputs.value.push('')
}

function removeRule(index: number) {
  rules.value.splice(index, 1)
  statusCodeInputs.value.splice(index, 1)
}

function handleClose() {
  emit('update:open', false)
}

function parseStatusCodes(input: string): { valid: true; codes?: number[] } | { valid: false; reason: string } {
  const trimmed = input.trim()
  if (!trimmed) return { valid: true }
  const parts = trimmed.split(/[,\s]+/)
  const codes: number[] = []
  for (const part of parts) {
    if (!part) continue
    if (!/^\d+$/.test(part)) return { valid: false, reason: `"${part}" 不是有效数字` }
    const value = parseInt(part, 10)
    if (value < 100 || value > 599) return { valid: false, reason: `${value} 不在 100-599 范围内` }
    codes.push(value)
  }
  return { valid: true, codes: codes.length > 0 ? codes : undefined }
}

function validatePattern(pattern: string): string | null {
  if (!pattern.trim()) return '正则表达式不能为空'
  try {
    new RegExp(pattern)
    return null
  } catch {
    return `无效的正则表达式: ${pattern}`
  }
}

async function handleSave() {
  if (!props.provider) return

  for (const rule of rules.value) {
    const err = validatePattern(rule.pattern)
    if (err) {
      showError(err, '验证失败')
      return
    }
  }

  for (let i = 0; i < rules.value.length; i++) {
    const result = parseStatusCodes(statusCodeInputs.value[i]?.trim() || '')
    if (!result.valid) {
      showError(`状态码格式错误: ${result.reason}，请输入 100-599 之间的整数，多个用逗号分隔`, '验证失败')
      return
    }
    rules.value[i].status_codes = result.codes
  }

  saving.value = true
  try {
    const filteredRules = rules.value.filter(rule => rule.pattern.trim())
    await updateProvider(props.provider.id, {
      error_passthrough_rules: filteredRules.length > 0
        ? { patterns: filteredRules }
        : null,
    })
    success('错误透传规则已保存')
    emit('saved')
    handleClose()
  } catch (err) {
    showError(parseApiError(err, '保存错误透传规则失败'), '保存失败')
  } finally {
    saving.value = false
  }
}
</script>
