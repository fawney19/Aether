<template>
  <Dialog
    :open="open"
    size="2xl"
    :title="dialogTitle"
    :description="dialogDescription"
    @update:open="(val: boolean) => { if (!val) emit('close') }"
  >
    <div
      v-if="showSetup"
      class="space-y-4"
    >
      <div class="space-y-2">
        <div class="flex items-center justify-between gap-3">
          <div class="text-sm font-medium">
            测试内容
          </div>
          <div class="text-[11px] text-muted-foreground">
            留空时使用默认提示词
          </div>
        </div>
        <Textarea
          :model-value="messageDraft"
          class="min-h-[132px]"
          placeholder="输入自定义测试内容；留空时使用系统默认测试提示词"
          @update:model-value="emit('update:messageDraft', $event)"
        />
      </div>

      <div
        v-if="showEndpointChoices"
        class="space-y-2"
      >
        <div class="text-sm font-medium">
          选择测试端点
        </div>
        <button
          v-for="endpoint in endpoints"
          :key="endpoint.id"
          type="button"
          class="w-full rounded-lg border border-border/60 px-3 py-3 text-left transition-colors hover:bg-muted/40"
          @click="emit('selectEndpoint', endpoint.id)"
        >
          <div class="flex items-center justify-between gap-3">
            <div class="min-w-0">
              <div class="text-sm font-medium">
                {{ formatApiFormat(endpoint.api_format) }}
              </div>
              <div class="mt-1 text-xs text-muted-foreground truncate">
                {{ endpoint.base_url }}
              </div>
            </div>
            <Badge variant="outline">
              {{ endpoint.is_active ? '已启用' : '已禁用' }}
            </Badge>
          </div>
        </button>
        <div class="text-[11px] text-muted-foreground">
          选择端点后会立即开始测试
        </div>
      </div>

      <div
        v-else
        class="space-y-4"
      >
        <div
          v-if="setupEndpoint"
          class="rounded-lg border border-border/60 bg-muted/20 p-4 space-y-2"
        >
          <div class="text-xs text-muted-foreground">
            本次测试端点
          </div>
          <div class="text-sm font-medium">
            {{ formatApiFormat(setupEndpoint.api_format) }}
          </div>
          <div class="text-xs text-muted-foreground break-all">
            {{ setupEndpoint.base_url }}
          </div>
        </div>

        <div
          v-else-if="mode === 'direct'"
          class="rounded-lg border border-border/60 bg-muted/20 px-3 py-2 text-xs text-muted-foreground"
        >
          该测试会直接在当前 Provider 内执行故障转移，并使用上方内容作为测试消息。
        </div>

        <Button
          class="w-full"
          @click="emit('start')"
        >
          开始测试
        </Button>
      </div>
    </div>

    <div
      v-else-if="testing"
      class="space-y-4 py-6"
    >
      <div class="flex flex-col items-center justify-center gap-3 text-center">
        <Loader2 class="w-8 h-8 animate-spin text-primary" />
        <div class="space-y-1">
          <p class="text-sm font-medium">
            正在测试模型
          </p>
          <p class="text-xs text-muted-foreground">
            {{ selectingModelName || '-' }}
          </p>
          <p
            v-if="selectedEndpoint"
            class="text-xs text-muted-foreground"
          >
            端点：{{ formatApiFormat(selectedEndpoint.api_format) }} · {{ selectedEndpoint.base_url }}
          </p>
        </div>
      </div>

      <div class="rounded-lg border border-border/60 bg-muted/20 p-4 space-y-4">
        <div class="space-y-2">
          <div class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
            <span>实时进度</span>
            <span>{{ liveTraceSummary.completed }}/{{ liveTraceSummary.total || 0 }}</span>
          </div>
          <div class="h-2 overflow-hidden rounded-full bg-muted">
            <div
              class="h-full bg-primary transition-all duration-300"
              :style="{ width: `${liveProgressPercent}%` }"
            />
          </div>
          <div class="flex flex-wrap gap-1.5">
            <Badge
              variant="secondary"
              class="text-[10px] px-1.5 py-0"
            >
              待执行 {{ liveTraceSummary.available }}
            </Badge>
            <Badge
              variant="outline"
              class="text-[10px] px-1.5 py-0"
            >
              进行中 {{ liveTraceSummary.pending }}
            </Badge>
            <Badge
              variant="success"
              class="text-[10px] px-1.5 py-0"
            >
              成功 {{ liveTraceSummary.success }}
            </Badge>
            <Badge
              variant="destructive"
              class="text-[10px] px-1.5 py-0"
            >
              失败 {{ liveTraceSummary.failed }}
            </Badge>
            <Badge
              variant="secondary"
              class="text-[10px] px-1.5 py-0"
            >
              跳过 {{ liveTraceSummary.skipped }}
            </Badge>
          </div>
        </div>

        <div class="grid gap-3 sm:grid-cols-2">
          <div class="rounded-md border border-border/60 bg-background/80 p-3 space-y-1">
            <div class="text-xs text-muted-foreground">
              测试账号
            </div>
            <div class="text-sm font-medium break-all">
              {{ liveAccountTitle }}
            </div>
            <div class="text-xs text-muted-foreground break-all">
              {{ liveAccountMeta }}
            </div>
          </div>
          <div class="rounded-md border border-border/60 bg-background/80 p-3 space-y-1">
            <div class="text-xs text-muted-foreground">
              实时状态
            </div>
            <div class="text-sm font-medium">
              {{ liveStatusTitle }}
            </div>
            <div class="text-xs text-muted-foreground break-all">
              {{ liveStatusDetail }}
            </div>
          </div>
        </div>

        <div
          v-if="requestId"
          class="text-[11px] text-muted-foreground break-all"
        >
          请求 ID：<code class="bg-muted px-1 py-0.5 rounded">{{ requestId }}</code>
        </div>

        <div
          v-if="liveRecentCandidates.length > 0"
          class="space-y-2"
        >
          <div class="text-xs font-medium text-muted-foreground">
            最近状态
          </div>
          <div class="space-y-2">
            <div
              v-for="candidate in liveRecentCandidates"
              :key="`${candidate.id}-${candidate.status}`"
              class="flex items-start justify-between gap-3 rounded-md border border-border/50 bg-background/70 px-3 py-2 text-xs"
            >
              <div class="min-w-0 space-y-1">
                <div class="flex items-center gap-2 min-w-0">
                  <span class="text-muted-foreground shrink-0">{{ formatTraceCandidateIndex(candidate) }}</span>
                  <Badge
                    :variant="statusVariant(candidate.status)"
                    class="text-[10px] px-1.5 py-0 shrink-0"
                  >
                    {{ statusDisplay(candidate) }}
                  </Badge>
                  <span class="truncate font-medium">{{ formatTraceCandidateAccount(candidate) }}</span>
                </div>
                <div class="text-muted-foreground break-all">
                  {{ traceCandidateDetail(candidate) }}
                </div>
              </div>
              <div class="shrink-0 text-muted-foreground tabular-nums">
                {{ candidate.latency_ms != null ? `${candidate.latency_ms}ms` : '' }}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div
      v-else-if="result"
      class="space-y-4"
    >
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2">
          <Badge :variant="result.success ? 'success' : 'destructive'">
            {{ result.success ? '成功' : '失败' }}
          </Badge>
          <span class="text-sm text-muted-foreground">
            {{ modeLabel }}
          </span>
        </div>
        <div class="text-xs text-muted-foreground">
          候选 {{ result.total_candidates }} / 尝试 {{ result.total_attempts }}
        </div>
      </div>

      <div class="text-sm space-y-1">
        <div>
          <span class="text-muted-foreground">请求模型: </span>
          <span class="font-medium">{{ result.model }}</span>
        </div>
        <div v-if="selectedEndpoint">
          <span class="text-muted-foreground">测试端点: </span>
          <span class="font-medium">{{ formatApiFormat(selectedEndpoint.api_format) }}</span>
          <span class="text-xs text-muted-foreground ml-1">{{ selectedEndpoint.base_url }}</span>
        </div>
        <div v-if="successEffectiveModel">
          <span class="text-muted-foreground">发送模型: </span>
          <span class="font-medium text-primary">{{ successEffectiveModel }}</span>
          <span
            v-if="successEffectiveModel !== result.model"
            class="text-xs text-muted-foreground ml-1"
          >(已映射)</span>
        </div>
      </div>

      <div
        v-if="result.error && !result.success"
        class="rounded-md bg-destructive/10 border border-destructive/20 px-3 py-2 text-xs text-destructive"
      >
        {{ result.error }}
      </div>

      <div
        v-if="attemptSummaryItems.length > 0"
        class="rounded-md border border-border/60 bg-muted/20 p-3 space-y-2"
      >
        <div class="text-xs font-medium text-muted-foreground">
          结果概览
        </div>
        <div class="flex flex-wrap gap-2">
          <div
            v-for="item in attemptSummaryItems"
            :key="item.key"
            class="flex items-center gap-2 rounded-md border border-border/60 bg-background/80 px-2.5 py-1.5 text-xs"
          >
            <Badge
              :variant="item.variant"
              class="text-[10px] px-1.5 py-0"
            >
              {{ item.count }}x
            </Badge>
            <span class="text-muted-foreground break-all">{{ item.label }}</span>
          </div>
        </div>
      </div>

      <div
        v-if="shouldCollapseAttempts"
        class="flex items-center justify-between gap-3 text-xs text-muted-foreground"
      >
        <span>仅展示前 {{ visibleAttempts.length }} 条，共 {{ resultAttempts.length }} 条</span>
        <Button
          variant="ghost"
          size="sm"
          @click="showAllAttempts = !showAllAttempts"
        >
          {{ showAllAttempts ? '收起详情' : `展开全部 ${resultAttempts.length} 条` }}
        </Button>
      </div>

      <!-- mobile: list layout -->
      <div
        v-if="resultAttempts.length > 0"
        class="space-y-2 sm:hidden"
      >
        <div
          v-for="(attempt, idx) in visibleAttempts"
          :key="'m' + idx"
          class="rounded-md border px-3 py-2 text-xs"
          :class="attemptRowClass(attempt.status)"
        >
          <div class="flex items-center justify-between gap-2">
            <div class="flex items-center gap-1.5 min-w-0">
              <span class="text-muted-foreground shrink-0">{{ formatAttemptIndex(attempt) }}</span>
              <Badge
                :variant="statusVariant(attempt.status)"
                class="text-[10px] px-1.5 py-0 shrink-0"
              >
                {{ statusDisplay(attempt) }}
              </Badge>
              <span
                v-if="attempt.latency_ms != null"
                class="text-muted-foreground shrink-0 tabular-nums"
              >
                {{ attempt.latency_ms }}ms
              </span>
            </div>
            <code
              v-if="showEndpointColumn"
              class="text-[11px] bg-muted px-1 py-0.5 rounded shrink-0"
            >{{ attempt.endpoint_api_format }}</code>
          </div>
          <div class="mt-1.5 space-y-0.5">
            <div
              v-if="attempt.key_name"
              class="font-medium truncate"
            >
              {{ attempt.key_name }}
            </div>
            <div class="text-muted-foreground">
              {{ maskKey(attempt.key_id) }}
            </div>
            <div
              v-if="hasEffectiveModel && attempt.effective_model"
              class="text-muted-foreground"
            >
              模型: {{ attempt.effective_model }}
            </div>
            <div
              v-if="attemptDetail(attempt) !== '-'"
              class="text-muted-foreground break-all mt-1"
            >
              {{ attemptDetail(attempt) }}
            </div>
          </div>
        </div>
      </div>

      <!-- desktop: table layout -->
      <div
        v-if="resultAttempts.length > 0"
        class="border rounded-md overflow-hidden hidden sm:block"
      >
        <table class="w-full text-xs table-fixed">
          <colgroup>
            <col class="w-8">
            <col class="w-[22%]">
            <col
              v-if="showEndpointColumn"
              class="w-20"
            >
            <col
              v-if="hasEffectiveModel"
              class="w-[16%]"
            >
            <col class="w-16">
            <col class="w-16">
            <col>
          </colgroup>
          <thead>
            <tr class="border-b bg-muted/30">
              <th class="pl-3 pr-1 py-2 text-left font-medium">
                #
              </th>
              <th class="px-3 py-2 text-left font-medium">
                Key
              </th>
              <th
                v-if="showEndpointColumn"
                class="px-3 py-2 text-left font-medium"
              >
                端点
              </th>
              <th
                v-if="hasEffectiveModel"
                class="px-3 py-2 text-left font-medium"
              >
                发送模型
              </th>
              <th class="px-3 py-2 text-left font-medium">
                状态
              </th>
              <th class="px-3 py-2 text-right font-medium">
                延迟
              </th>
              <th class="px-3 py-2 text-left font-medium">
                详情
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="(attempt, idx) in visibleAttempts"
              :key="idx"
              class="border-b last:border-b-0 align-top"
              :class="attemptRowClass(attempt.status)"
            >
              <td class="pl-3 pr-1 py-2 text-muted-foreground">
                {{ formatAttemptIndex(attempt) }}
              </td>
              <td class="px-3 py-2">
                <div
                  v-if="attempt.key_name"
                  class="font-medium truncate"
                  :title="attempt.key_name"
                >
                  {{ attempt.key_name }}
                </div>
                <div
                  class="text-muted-foreground truncate"
                  :title="attempt.key_id"
                >
                  {{ maskKey(attempt.key_id) }}
                </div>
              </td>
              <td
                v-if="showEndpointColumn"
                class="px-3 py-2"
              >
                <code class="text-[11px] bg-muted px-1 py-0.5 rounded">{{ attempt.endpoint_api_format }}</code>
              </td>
              <td
                v-if="hasEffectiveModel"
                class="px-3 py-2 truncate"
                :title="attempt.effective_model || '-'"
              >
                {{ attempt.effective_model || '-' }}
              </td>
              <td class="px-3 py-2">
                <Badge
                  :variant="statusVariant(attempt.status)"
                  class="text-[10px] px-1.5 py-0"
                >
                  {{ statusDisplay(attempt) }}
                </Badge>
              </td>
              <td class="px-3 py-2 text-right text-muted-foreground tabular-nums">
                {{ attempt.latency_ms != null ? attempt.latency_ms + 'ms' : '-' }}
              </td>
              <td class="px-3 py-2 text-muted-foreground">
                <div
                  class="break-all line-clamp-2"
                  :title="attemptDetail(attempt)"
                >
                  {{ attemptDetail(attempt) }}
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div
        v-else
        class="text-center text-sm text-muted-foreground py-4"
      >
        没有可用的候选进行测试
      </div>
    </div>

    <template #footer>
      <Button
        v-if="showResult && canReselect"
        variant="outline"
        size="sm"
        @click="emit('back')"
      >
        重新选择端点
      </Button>
      <Button
        variant="outline"
        size="sm"
        @click="emit('close')"
      >
        {{ showSetup ? '取消' : '关闭' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Loader2 } from 'lucide-vue-next'
import { Dialog, Badge } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Textarea from '@/components/ui/textarea.vue'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import type { TestModelFailoverResponse, TestAttemptDetail } from '@/api/endpoints/providers'
import type { CandidateRecord, RequestTrace } from '@/api/requestTrace'

type TestEndpointOption = {
  id: string
  api_format: string
  base_url: string
  is_active: boolean
}

const props = defineProps<{
  open: boolean
  result: TestModelFailoverResponse | null
  mode?: 'global' | 'direct'
  selectingModelName?: string | null
  endpoints?: TestEndpointOption[]
  selectedEndpoint?: TestEndpointOption | null
  testing?: boolean
  trace?: RequestTrace | null
  requestId?: string | null
  showEndpointSelector?: boolean
  messageDraft?: string
}>()

const emit = defineEmits<{
  close: []
  back: []
  start: []
  selectEndpoint: [endpointId: string]
  'update:messageDraft': [value: string]
}>()

const endpoints = computed(() => props.endpoints ?? [])
const traceCandidates = computed(() => props.trace?.candidates ?? [])
const messageDraft = computed(() => props.messageDraft ?? '')
const showSetup = computed(() => props.open && !props.testing && !props.result)
const showEndpointChoices = computed(() => showSetup.value && !!props.showEndpointSelector && endpoints.value.length > 1)
const showResult = computed(() => !!props.result)
const canReselect = computed(() => !!props.showEndpointSelector && endpoints.value.length > 1)
const setupEndpoint = computed(() => {
  if (props.selectedEndpoint) return props.selectedEndpoint
  if (endpoints.value.length === 1) return endpoints.value[0]
  return null
})

const dialogTitle = computed(() => {
  if (props.result) return '模型测试结果'
  return '模型测试'
})

const dialogDescription = computed(() => {
  if (showEndpointChoices.value && props.selectingModelName) {
    return `为 ${props.selectingModelName} 选择端点`
  }
  if (showSetup.value && props.selectingModelName) {
    return `为 ${props.selectingModelName} 设置测试内容`
  }
  if (props.testing && props.selectedEndpoint) {
    return `正在通过 ${formatApiFormat(props.selectedEndpoint.api_format)} 测试 ${props.selectingModelName || '模型'}`
  }
  return ''
})

const modeLabel = computed(() => {
  if (props.mode === 'global') return '模拟外部请求'
  if (props.mode === 'direct') return '直接测试'
  return ''
})

const successEffectiveModel = computed(() => {
  if (!props.result) return null
  const successAttempt = props.result.attempts.find(a => a.status === 'success')
  return successAttempt?.effective_model || null
})

const hasEffectiveModel = computed(() => {
  if (!props.result) return false
  return props.result.attempts.some(a => a.effective_model && a.effective_model !== props.result?.model)
})

const showEndpointColumn = computed(() => {
  if (!props.result) return false
  if (props.mode === 'direct') return true
  const formats = new Set(props.result.attempts.map(a => a.endpoint_api_format))
  return formats.size > 1
})

const resultAttempts = computed(() => props.result?.attempts ?? [])
const showAllAttempts = ref(false)

watch(() => props.result, () => {
  showAllAttempts.value = false
})

const shouldCollapseAttempts = computed(() => resultAttempts.value.length > 20)

const visibleAttempts = computed(() => {
  if (!shouldCollapseAttempts.value || showAllAttempts.value) {
    return resultAttempts.value
  }
  return resultAttempts.value.slice(0, 20)
})

type AttemptSummaryItem = {
  key: string
  label: string
  count: number
  variant: 'success' | 'destructive' | 'secondary'
}

const attemptSummaryItems = computed<AttemptSummaryItem[]>(() => {
  const groups = new Map<string, AttemptSummaryItem>()

  for (const attempt of resultAttempts.value) {
    const label = summarizeAttempt(attempt)
    const key = `${attempt.status}:${label}`
    const existing = groups.get(key)
    if (existing) {
      existing.count += 1
      continue
    }
    groups.set(key, {
      key,
      label,
      count: 1,
      variant: statusVariant(attempt.status),
    })
  }

  const variantRank: Record<AttemptSummaryItem['variant'], number> = {
    destructive: 0,
    secondary: 1,
    success: 2,
  }

  return [...groups.values()].sort((left, right) => {
    if (right.count !== left.count) return right.count - left.count
    return variantRank[left.variant] - variantRank[right.variant]
  })
})

const liveTraceSummary = computed(() => {
  const summary = {
    total: traceCandidates.value.length,
    available: 0,
    pending: 0,
    success: 0,
    failed: 0,
    skipped: 0,
    completed: 0,
  }

  for (const candidate of traceCandidates.value) {
    if (candidate.status === 'available' || candidate.status === 'unused') summary.available += 1
    if (candidate.status === 'pending' || candidate.status === 'streaming') summary.pending += 1
    if (candidate.status === 'success') summary.success += 1
    if (candidate.status === 'failed' || candidate.status === 'cancelled' || candidate.status === 'stream_interrupted') summary.failed += 1
    if (candidate.status === 'skipped') summary.skipped += 1
  }

  summary.completed = summary.success + summary.failed + summary.skipped
  return summary
})

const liveProgressPercent = computed(() => {
  if (liveTraceSummary.value.total <= 0) return 6
  const raw = Math.round((liveTraceSummary.value.completed / liveTraceSummary.value.total) * 100)
  return Math.min(100, Math.max(raw, liveTraceSummary.value.pending > 0 ? 12 : 6))
})

const activeTraceCandidate = computed(() => {
  const preferredStatuses = ['pending', 'streaming', 'failed', 'success', 'skipped', 'cancelled']
  for (let index = traceCandidates.value.length - 1; index >= 0; index -= 1) {
    const candidate = traceCandidates.value[index]
    if (preferredStatuses.includes(candidate.status)) return candidate
  }
  return traceCandidates.value[0] ?? null
})

const liveAccountTitle = computed(() => {
  const candidate = activeTraceCandidate.value
  if (!candidate) return '等待分配测试账号'
  return candidate.key_account_label || candidate.key_name || candidate.key_preview || '等待分配测试账号'
})

const liveAccountMeta = computed(() => {
  const candidate = activeTraceCandidate.value
  if (!candidate) return '候选创建后会显示测试账号和认证方式'
  const parts: string[] = []
  if (candidate.key_auth_type) parts.push(formatAuthType(candidate.key_auth_type))
  if (candidate.key_oauth_plan_type) parts.push(candidate.key_oauth_plan_type)
  if (candidate.key_preview && candidate.key_preview !== candidate.key_account_label) parts.push(candidate.key_preview)
  return parts.join(' · ') || '正在等待候选进入执行阶段'
})

const liveStatusTitle = computed(() => {
  const candidate = activeTraceCandidate.value
  if (!candidate) return '正在创建测试请求'
  if (candidate.status === 'pending' || candidate.status === 'streaming') {
    return `正在测试 ${formatTraceCandidateIndex(candidate)}`
  }
  return statusLabel(candidate.status)
})

const liveStatusDetail = computed(() => {
  const candidate = activeTraceCandidate.value
  if (!candidate) return '等待后端写入候选状态'
  return traceCandidateDetail(candidate)
})

const liveRecentCandidates = computed(() => {
  return traceCandidates.value
    .filter(candidate => !['available', 'unused'].includes(candidate.status))
    .slice(-4)
    .reverse()
})

function statusVariant(status: string) {
  if (status === 'success') return 'success' as const
  if (status === 'failed' || status === 'stream_interrupted') return 'destructive' as const
  return 'secondary' as const
}

function statusLabel(status: string) {
  if (status === 'success') return '成功'
  if (status === 'failed') return '失败'
  if (status === 'skipped') return '跳过'
  if (status === 'pending') return '等待中'
  if (status === 'streaming') return '测试中'
  if (status === 'cancelled') return '已取消'
  if (status === 'stream_interrupted') return '流中断'
  if (status === 'available') return '待执行'
  return status
}

function statusDisplay(item: { status: string; status_code?: number | null }): string {
  const code = item.status_code
  const status = item.status
  if (!code) return statusLabel(status)
  // 失败但 HTTP 状态码是 2xx：显示 "200 体内错误" 以区分
  if (status === 'failed' && code >= 200 && code < 300) {
    return `${code} 体内错误`
  }
  return String(code)
}

function compactDetail(value: string | null | undefined, maxLength = 64): string | null {
  if (!value) return null
  const compact = value.replace(/\s+/g, ' ').trim()
  if (!compact) return null
  return compact.length > maxLength ? `${compact.slice(0, maxLength)}…` : compact
}

function summarizeAttempt(attempt: TestAttemptDetail): string {
  if (attempt.status === 'skipped') return '跳过'
  if (attempt.status === 'cancelled') return '已取消'
  if (attempt.status === 'success') return '成功'

  const detail = compactDetail(attempt.error_message || attempt.skip_reason)
  if (attempt.status_code != null) {
    if (detail) return `${attempt.status_code} ${detail}`
    if (attempt.status === 'failed' && attempt.status_code >= 200 && attempt.status_code < 300) {
      return `${attempt.status_code} 体内错误`
    }
    return `${attempt.status_code} ${statusLabel(attempt.status)}`
  }
  return detail || statusLabel(attempt.status)
}

function attemptRowClass(status: string) {
  if (status === 'success') return 'bg-green-500/5'
  if (status === 'failed') return 'bg-red-500/5'
  if (status === 'cancelled') return 'bg-amber-500/5'
  if (status === 'skipped') return 'bg-muted/20'
  return ''
}

function maskKey(key: string): string {
  if (key.length <= 8) return key
  return `${key.slice(0, 4)}...${key.slice(-4)}`
}

function formatAuthType(authType: string): string {
  const lowered = authType.toLowerCase()
  if (lowered === 'api_key') return 'API Key'
  if (lowered === 'service_account') return 'Service Account'
  if (lowered === 'oauth') return 'OAuth'
  if (lowered === 'codex') return 'Codex OAuth'
  if (lowered === 'antigravity') return 'Antigravity OAuth'
  if (lowered === 'kiro') return 'Kiro OAuth'
  return authType
}

function formatAttemptIndex(attempt: TestAttemptDetail): string {
  const retryIndex = attempt.retry_index ?? 0
  return retryIndex > 0 ? `#${attempt.candidate_index}.${retryIndex}` : `#${attempt.candidate_index}`
}

function formatTraceCandidateIndex(candidate: CandidateRecord): string {
  return candidate.retry_index > 0 ? `#${candidate.candidate_index}.${candidate.retry_index}` : `#${candidate.candidate_index}`
}

function formatTraceCandidateAccount(candidate: CandidateRecord): string {
  return candidate.key_account_label || candidate.key_name || candidate.key_preview || '待分配账号'
}

function traceCandidateDetail(candidate: CandidateRecord): string {
  if (candidate.skip_reason) return candidate.skip_reason
  if (candidate.error_message) return candidate.error_message
  if (candidate.endpoint_name) return `端点：${formatApiFormat(candidate.endpoint_name)}`
  return '等待响应中…'
}

function attemptDetail(attempt: TestAttemptDetail): string {
  if (attempt.status === 'cancelled') return '测试已取消'
  if (attempt.skip_reason) return attempt.skip_reason
  if (attempt.error_message) return attempt.error_message
  if (attempt.status === 'success') return attempt.endpoint_base_url
  return '-'
}
</script>
