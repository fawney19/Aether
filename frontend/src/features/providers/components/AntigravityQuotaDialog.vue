<template>
  <Dialog
    :model-value="open"
    :title="`配额详情 - ${keyName}`"
    :icon="BarChart3"
    size="2xl"
    :z-index="70"
    @update:model-value="$emit('update:open', $event)"
  >
    <template
      v-if="providerId && items.length > 0"
      #header-actions
    >
      <DropdownMenu :modal="false">
        <DropdownMenuTrigger as-child>
          <Button
            variant="ghost"
            size="icon"
            class="h-8 w-8"
            title="测试模型"
            :disabled="!!testingModel"
          >
            <Loader2
              v-if="testingModel"
              class="w-3.5 h-3.5 animate-spin"
            />
            <Play
              v-else
              class="w-3.5 h-3.5"
            />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem
            v-for="item in items"
            :key="item.model"
            @select="handleTestModel(item.model)"
          >
            {{ item.model }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </template>

    <div class="py-2">
      <div
        v-if="items.length > 0"
        class="grid grid-cols-2 gap-3"
      >
        <div
          v-for="item in items"
          :key="item.model"
        >
          <div class="flex items-center justify-between text-[10px] mb-0.5">
            <span
              class="text-muted-foreground truncate mr-2 min-w-0 flex-1"
              :title="item.model"
            >
              {{ item.label }}
            </span>
            <span :class="getQuotaRemainingClass(item.usedPercent)">
              {{ item.remainingPercent.toFixed(1) }}%
            </span>
          </div>
          <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
            <div
              class="absolute left-0 top-0 h-full transition-all duration-300"
              :class="getQuotaRemainingBarColor(item.usedPercent)"
              :style="{ width: `${Math.max(item.remainingPercent, 0)}%` }"
            />
          </div>
          <div
            v-if="item.resetSeconds !== null"
            class="text-[9px] text-muted-foreground/70 mt-0.5"
          >
            <template v-if="item.resetSeconds > 0">
              {{ formatResetTime(item.resetSeconds) }}后重置
            </template>
            <template v-else>
              已重置
            </template>
          </div>
        </div>
      </div>
      <div
        v-else
        class="text-center text-sm text-muted-foreground py-8"
      >
        暂无配额数据
      </div>
    </div>
    <template #footer>
      <Button
        variant="outline"
        @click="$emit('update:open', false)"
      >
        关闭
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { BarChart3, Play, Loader2 } from 'lucide-vue-next'
import { Dialog } from '@/components/ui'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from '@/components/ui'
import Button from '@/components/ui/button.vue'
import { testModel } from '@/api/endpoints/providers'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'

const props = defineProps<{
  open: boolean
  metadata: Record<string, unknown> | null
  keyName: string
  providerId?: string
  keyId?: string
}>()

defineEmits<{
  'update:open': [value: boolean]
}>()

interface QuotaItem {
  model: string
  label: string
  usedPercent: number
  remainingPercent: number
  resetSeconds: number | null
}

const { error: showError, success: showSuccess } = useToast()
const testingModel = ref<string | null>(null)

const items = computed<QuotaItem[]>(() => {
  const antigravity = props.metadata?.antigravity
  if (!antigravity || typeof antigravity !== 'object') return []
  const quotaByModel = (antigravity as Record<string, unknown>).quota_by_model
  if (!quotaByModel || typeof quotaByModel !== 'object') return []

  const result: QuotaItem[] = []
  for (const [model, rawInfo] of Object.entries(quotaByModel as Record<string, unknown>)) {
    if (!model) continue
    const info = (rawInfo || {}) as Record<string, unknown>

    let usedPercent = Number(info['used_percent'])
    if (!Number.isFinite(usedPercent)) {
      const remainingFraction = Number(info['remaining_fraction'])
      if (Number.isFinite(remainingFraction)) {
        usedPercent = (1 - remainingFraction) * 100
      } else {
        continue
      }
    }

    if (usedPercent < 0) usedPercent = 0
    if (usedPercent > 100) usedPercent = 100

    const remainingPercent = Math.max(100 - usedPercent, 0)

    let resetSeconds: number | null = null
    const resetTime = info['reset_time']
    if (typeof resetTime === 'string' && resetTime.trim()) {
      const ts = Date.parse(resetTime.trim())
      if (!Number.isNaN(ts)) {
        const diff = Math.floor((ts - Date.now()) / 1000)
        resetSeconds = diff > 0 ? diff : 0
      }
    }

    result.push({ model, label: model, usedPercent, remainingPercent, resetSeconds })
  }

  result.sort((a, b) => (b.usedPercent - a.usedPercent) || a.model.localeCompare(b.model))
  return result
})

async function handleTestModel(modelName: string) {
  if (!props.providerId || testingModel.value) return

  testingModel.value = modelName

  try {
    const result = await testModel({
      provider_id: props.providerId,
      model_name: modelName,
      api_key_id: props.keyId,
      api_format: 'gemini:chat',
    })

    if (result.success) {
      const content =
        result.data?.response?.choices?.[0]?.message?.content
        || result.data?.content_preview
      if (content) {
        showSuccess(`测试成功，响应: ${String(content).substring(0, 100)}${String(content).length > 100 ? '...' : ''}`)
      } else {
        showSuccess(`模型 "${modelName}" 测试成功`)
      }
    } else {
      showError(`模型测试失败: ${result.error || '未知错误'}`)
    }
  } catch (err: unknown) {
    showError(`模型测试失败: ${parseApiError(err, '测试请求失败')}`)
  } finally {
    testingModel.value = null
  }
}

function getQuotaRemainingClass(usedPercent: number): string {
  const remaining = 100 - usedPercent
  if (remaining <= 10) return 'text-red-600 dark:text-red-400'
  if (remaining <= 30) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-green-600 dark:text-green-400'
}

function getQuotaRemainingBarColor(usedPercent: number): string {
  const remaining = 100 - usedPercent
  if (remaining <= 10) return 'bg-red-500 dark:bg-red-400'
  if (remaining <= 30) return 'bg-yellow-500 dark:bg-yellow-400'
  return 'bg-green-500 dark:bg-green-400'
}

function formatResetTime(seconds: number): string {
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)

  if (days > 0) return `${days}天 ${hours}小时`
  if (hours > 0) return `${hours}小时 ${minutes}分钟`
  return `${minutes}分钟`
}
</script>
