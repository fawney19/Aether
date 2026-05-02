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
import type { UpstreamMetadata, QuotaStatusSnapshot, QuotaWindowSnapshot } from '@/api/endpoints/types'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'

const props = defineProps<{
  open: boolean
  metadata: UpstreamMetadata | null
  quotaSnapshot?: QuotaStatusSnapshot | null
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

function getQuotaSnapshotUpdatedAt(quota: QuotaStatusSnapshot | null | undefined): number | undefined {
  const updatedAt = quota?.updated_at ?? quota?.observed_at
  return typeof updatedAt === 'number' ? updatedAt : undefined
}

function getQuotaWindowLiveResetSeconds(
  quota: QuotaStatusSnapshot | null | undefined,
  window: QuotaWindowSnapshot | null | undefined,
): number | null {
  if (!window) return null

  const now = Math.floor(Date.now() / 1000)
  if (typeof window.reset_at === 'number') {
    return Math.max(window.reset_at - now, 0)
  }

  if (typeof window.reset_seconds === 'number') {
    const updatedAt = getQuotaSnapshotUpdatedAt(quota)
    const elapsed = typeof updatedAt === 'number' ? Math.max(now - updatedAt, 0) : 0
    return Math.max(window.reset_seconds - elapsed, 0)
  }

  return null
}

function buildItemsFromQuotaSnapshot(quota: QuotaStatusSnapshot | null | undefined): QuotaItem[] {
  if (!quota) return []

  const providerType = String(quota.provider_type || '').trim().toLowerCase()
  if (providerType && providerType !== 'antigravity') return []

  const windows = Array.isArray(quota.windows)
    ? quota.windows.filter(window => String(window?.scope || '').trim().toLowerCase() === 'model')
    : []
  if (windows.length === 0) return []

  const items = windows
    .map((window) => {
      const model = String(window.model || window.label || window.code || '').trim()
      if (!model) return null

      const usedPercent =
        typeof window.used_ratio === 'number'
          ? Math.max(Math.min(window.used_ratio * 100, 100), 0)
          : typeof window.remaining_ratio === 'number'
            ? Math.max(Math.min((1 - window.remaining_ratio) * 100, 100), 0)
            : null
      if (usedPercent == null) return null

      const remainingPercent =
        typeof window.remaining_ratio === 'number'
          ? Math.max(Math.min(window.remaining_ratio * 100, 100), 0)
          : Math.max(100 - usedPercent, 0)

      return {
        model,
        label: String(window.label || window.model || model),
        usedPercent,
        remainingPercent,
        resetSeconds: getQuotaWindowLiveResetSeconds(quota, window),
      } satisfies QuotaItem
    })
    .filter((item): item is QuotaItem => item !== null)

  items.sort((a, b) => (b.usedPercent - a.usedPercent) || a.model.localeCompare(b.model))
  return items
}

const items = computed<QuotaItem[]>(() => {
  const snapshotItems = buildItemsFromQuotaSnapshot(props.quotaSnapshot)
  if (snapshotItems.length > 0) return snapshotItems

  const antigravity = props.metadata?.antigravity
  if (!antigravity || typeof antigravity !== 'object') return []
  const quotaByModel = antigravity.quota_by_model
  if (!quotaByModel || typeof quotaByModel !== 'object') return []

  const result: QuotaItem[] = []
  for (const [model, rawInfo] of Object.entries(quotaByModel)) {
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
      api_format: 'gemini:generate_content',
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
