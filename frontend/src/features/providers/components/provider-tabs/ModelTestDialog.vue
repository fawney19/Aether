<template>
  <Dialog
    :open="open"
    size="3xl"
    :close-on-backdrop="false"
    @update:open="(val: boolean) => { if (!val) emit('close') }"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="space-y-3">
          <div class="text-lg font-semibold text-foreground leading-tight">
            {{ dialogTitle }}
          </div>
          <p
            v-if="dialogDescription && !showResult"
            class="text-xs text-muted-foreground"
          >
            {{ dialogDescription }}
          </p>
        </div>
      </div>
    </template>

    <div
      v-if="showSetup"
      class="space-y-4"
    >
      <div
        v-if="endpoints.length > 0"
        class="space-y-2"
      >
        <div class="flex items-center justify-between gap-3">
          <div class="text-sm font-medium">
            选择测试端点
          </div>
          <div class="text-[11px] text-muted-foreground">
            当前测试会固定到选中的端点
          </div>
        </div>
        <div class="grid gap-2 md:grid-cols-2">
          <button
            v-for="endpoint in endpoints"
            :key="endpoint.id"
            type="button"
            class="h-full w-full rounded-lg border px-3 py-3 text-left transition-colors"
            :class="selectedEndpoint?.id === endpoint.id
              ? 'border-primary bg-primary/5'
              : 'border-border/60 hover:bg-muted/40'"
            @click="emit('selectEndpoint', endpoint.id)"
          >
            <div class="flex items-center justify-between gap-3">
              <div class="min-w-0">
                <div class="text-sm font-medium">
                  {{ formatApiFormat(endpoint.api_format) }}
                </div>
                <div class="mt-1 text-xs text-muted-foreground break-all">
                  {{ endpoint.base_url }}
                </div>
              </div>
              <Badge :variant="selectedEndpoint?.id === endpoint.id ? 'success' : 'outline'">
                {{ selectedEndpoint?.id === endpoint.id ? '已选择' : (endpoint.is_active ? '可用' : '已禁用') }}
              </Badge>
            </div>
          </button>
        </div>
      </div>

      <div class="grid gap-4 lg:grid-cols-2 lg:items-start">
        <div class="space-y-2">
          <div class="flex items-center justify-between gap-3">
            <div class="text-sm font-medium">
              测试请求头
            </div>
            <div class="flex items-center gap-1">
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8 rounded-lg text-muted-foreground"
                title="格式化请求头 JSON"
                @click="formatRequestHeadersDraft"
              >
                <Code2 class="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8 rounded-lg text-muted-foreground"
                title="重置请求头"
                @click="resetRequestHeadersDraft"
              >
                <RotateCcw class="h-4 w-4" />
              </Button>
            </div>
          </div>
          <Textarea
            :model-value="requestHeadersDraft"
            class="min-h-[260px] font-mono text-xs"
            placeholder="输入 JSON 请求头"
            @update:model-value="emit('update:requestHeadersDraft', $event)"
          />
          <div
            v-if="requestHeadersError"
            class="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {{ requestHeadersError }}
          </div>
          <div class="rounded-md border border-border/60 bg-muted/20 px-3 py-2 text-[11px] text-muted-foreground">
            这里的请求头会合并到测试请求里；鉴权头和必要系统头仍由后端按端点规则补齐。
          </div>
        </div>

        <div class="space-y-2">
          <div class="flex items-center justify-between gap-3">
            <div class="text-sm font-medium">
              测试请求体
            </div>
            <div class="flex items-center gap-1">
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8 rounded-lg text-muted-foreground"
                title="格式化请求体 JSON"
                @click="formatRequestBodyDraft"
              >
                <Code2 class="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8 rounded-lg text-muted-foreground"
                title="重置请求体"
                @click="resetRequestBodyDraft"
              >
                <RotateCcw class="h-4 w-4" />
              </Button>
            </div>
          </div>
          <Textarea
            :model-value="requestBodyDraft"
            class="min-h-[260px] font-mono text-xs"
            placeholder="输入 JSON 请求体"
            @update:model-value="emit('update:requestBodyDraft', $event)"
          />
          <div
            v-if="requestBodyError"
            class="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {{ requestBodyError }}
          </div>
          <div class="rounded-md border border-border/60 bg-muted/20 px-3 py-2 text-[11px] text-muted-foreground">
            会强制使用当前测试模型；这里编辑的是测试基础请求体，实际发送时会按端点格式转换并应用规则。
          </div>
        </div>
      </div>

      <Button
        class="w-full"
        :disabled="startDisabled"
        @click="emit('start')"
      >
        开始测试
      </Button>
    </div>

    <div
      v-else-if="testing"
      class="space-y-4 py-6"
    >
      <div class="flex flex-col items-center justify-center gap-3 text-center">
        <Loader2 class="h-8 w-8 animate-spin text-primary" />
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
              class="px-1.5 py-0 text-[10px]"
            >
              待执行 {{ liveTraceSummary.available }}
            </Badge>
            <Badge
              variant="outline"
              class="px-1.5 py-0 text-[10px]"
            >
              进行中 {{ liveTraceSummary.pending }}
            </Badge>
            <Badge
              variant="success"
              class="px-1.5 py-0 text-[10px]"
            >
              成功 {{ liveTraceSummary.success }}
            </Badge>
            <Badge
              variant="destructive"
              class="px-1.5 py-0 text-[10px]"
            >
              失败 {{ liveTraceSummary.failed }}
            </Badge>
            <Badge
              variant="secondary"
              class="px-1.5 py-0 text-[10px]"
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
            <div class="break-all text-sm font-medium">
              {{ liveAccountTitle }}
            </div>
            <div class="break-all text-xs text-muted-foreground">
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
            <div class="break-all text-xs text-muted-foreground">
              {{ liveStatusDetail }}
            </div>
          </div>
        </div>

        <div
          v-if="requestId"
          class="break-all text-[11px] text-muted-foreground"
        >
          请求 ID：<code class="rounded bg-muted px-1 py-0.5">{{ requestId }}</code>
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
                <div class="flex min-w-0 items-center gap-2">
                  <span class="shrink-0 text-muted-foreground">{{ formatTraceCandidateIndex(candidate) }}</span>
                  <Badge
                    :variant="statusVariant(candidate.status)"
                    class="shrink-0 px-1.5 py-0 text-[10px]"
                  >
                    {{ statusDisplay(candidate) }}
                  </Badge>
                  <span class="truncate font-medium">{{ formatTraceCandidateAccount(candidate) }}</span>
                </div>
                <div class="break-all text-muted-foreground">
                  {{ traceCandidateDetail(candidate) }}
                </div>
              </div>
              <div class="shrink-0 tabular-nums text-muted-foreground">
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
      <HorizontalRequestTimeline
        v-if="showTraceTimeline && requestId"
        :request-id="requestId"
        :trace-data="trace"
        :request-api-format="timelineRequestApiFormat"
        @select-attempt="handleTraceAttemptSelect"
      />

      <div
        v-if="!showTraceTimeline && shouldCollapseAttempts"
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

      <div
        v-if="!showTraceTimeline && resultAttempts.length > 0"
        class="space-y-2 sm:hidden"
      >
        <div
          v-for="(attempt, idx) in visibleAttempts"
          :key="'m' + idx"
          class="rounded-md border px-3 py-2 text-xs"
          :class="attemptRowClass(attempt.status)"
        >
          <div class="flex items-center justify-between gap-2">
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="shrink-0 text-muted-foreground">{{ formatAttemptIndex(attempt) }}</span>
              <Badge
                :variant="statusVariant(attempt.status)"
                class="shrink-0 px-1.5 py-0 text-[10px]"
              >
                {{ statusDisplay(attempt) }}
              </Badge>
              <span
                v-if="attempt.latency_ms != null"
                class="shrink-0 tabular-nums text-muted-foreground"
              >
                {{ attempt.latency_ms }}ms
              </span>
            </div>
            <code
              v-if="showEndpointColumn"
              class="shrink-0 rounded bg-muted px-1 py-0.5 text-[11px]"
            >{{ attempt.endpoint_api_format }}</code>
          </div>
          <div class="mt-1.5 space-y-0.5">
            <div
              v-if="attempt.key_name"
              class="truncate font-medium"
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
              class="mt-1 break-all text-muted-foreground"
            >
              {{ attemptDetail(attempt) }}
            </div>
          </div>
        </div>
      </div>

      <div
        v-if="!showTraceTimeline && resultAttempts.length > 0"
        class="hidden overflow-hidden rounded-md border sm:block"
      >
        <table class="w-full table-fixed text-xs">
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
              <th class="py-2 pl-3 pr-1 text-left font-medium">
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
              class="last:border-b-0 align-top border-b"
              :class="attemptRowClass(attempt.status)"
            >
              <td class="py-2 pl-3 pr-1 text-muted-foreground">
                {{ formatAttemptIndex(attempt) }}
              </td>
              <td class="px-3 py-2">
                <div
                  v-if="attempt.key_name"
                  class="truncate font-medium"
                  :title="attempt.key_name"
                >
                  {{ attempt.key_name }}
                </div>
                <div
                  class="truncate text-muted-foreground"
                  :title="attempt.key_id"
                >
                  {{ maskKey(attempt.key_id) }}
                </div>
              </td>
              <td
                v-if="showEndpointColumn"
                class="px-3 py-2"
              >
                <code class="rounded bg-muted px-1 py-0.5 text-[11px]">{{ attempt.endpoint_api_format }}</code>
              </td>
              <td
                v-if="hasEffectiveModel"
                class="truncate px-3 py-2"
                :title="attempt.effective_model || '-'"
              >
                {{ attempt.effective_model || '-' }}
              </td>
              <td class="px-3 py-2">
                <Badge
                  :variant="statusVariant(attempt.status)"
                  class="px-1.5 py-0 text-[10px]"
                >
                  {{ statusDisplay(attempt) }}
                </Badge>
              </td>
              <td class="px-3 py-2 text-right tabular-nums text-muted-foreground">
                {{ attempt.latency_ms != null ? attempt.latency_ms + 'ms' : '-' }}
              </td>
              <td class="px-3 py-2 text-muted-foreground">
                <div
                  class="line-clamp-2 break-all"
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
        v-else-if="!showTraceTimeline"
        class="py-4 text-center text-sm text-muted-foreground"
      >
        {{ resultEmptyMessage }}
      </div>

      <div
        v-if="showDebugInspector"
        class="space-y-3"
      >
        <div
          v-if="!showTraceTimeline && inspectableAttempts.length > 0"
          class="flex flex-wrap gap-2"
        >
          <Button
            v-for="attempt in inspectableAttempts"
            :key="inspectionKey(attempt)"
            size="sm"
            :variant="selectedInspectionKey === inspectionKey(attempt) ? 'default' : 'outline'"
            @click="selectedInspectionKey = inspectionKey(attempt); selectedTraceCandidate = null"
          >
            {{ formatAttemptIndex(attempt) }} · {{ statusLabel(attempt.status) }}
          </Button>
        </div>

        <div
          v-if="selectedInspectionAttempt"
          class="space-y-3"
        >
          <div
            v-if="!showTraceTimeline"
            class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground"
          >
            <span class="font-medium text-foreground">
              {{ formatAttemptIndex(selectedInspectionAttempt) }}
            </span>
            <span>{{ selectedInspectionAttempt.key_name || maskKey(selectedInspectionAttempt.key_id) }}</span>
            <span>·</span>
            <span>{{ formatApiFormat(selectedInspectionAttempt.endpoint_api_format) }}</span>
          </div>

          <Card>
            <div class="p-3 sm:p-4">
              <Tabs
                v-model="inspectionTab"
                :default-value="inspectionTab"
              >
                <div class="flex items-center border-b pb-2 mb-3">
                  <button
                    v-for="tab in detailTabs"
                    :key="tab.name"
                    class="px-2 sm:px-3 py-1.5 text-sm transition-colors border-b-2 -mb-[9px] whitespace-nowrap"
                    :class="inspectionTab === tab.name
                      ? 'border-primary text-foreground font-medium'
                      : 'border-transparent text-muted-foreground hover:text-foreground'"
                    @click="inspectionTab = tab.name"
                  >
                    {{ tab.label }}
                  </button>
                </div>

                <div class="content-block rounded-md border overflow-hidden">
                  <div class="flex items-center justify-end gap-0.5 px-3 py-1 border-b bg-muted/40">
                    <button
                      :title="inspectionExpandDepth === 0 ? '展开全部' : '收缩全部'"
                      class="p-1 rounded transition-colors text-muted-foreground hover:bg-muted"
                      @click="inspectionExpandDepth === 0 ? expandInspectionContent() : collapseInspectionContent()"
                    >
                      <Maximize2
                        v-if="inspectionExpandDepth === 0"
                        class="w-3.5 h-3.5"
                      />
                      <Minimize2
                        v-else
                        class="w-3.5 h-3.5"
                      />
                    </button>

                    <button
                      :title="inspectionCopiedStates[inspectionTab] ? '已复制' : '复制'"
                      class="p-1 rounded transition-colors text-muted-foreground hover:bg-muted"
                      @click="copyInspectionContent(inspectionTab)"
                    >
                      <Check
                        v-if="inspectionCopiedStates[inspectionTab]"
                        class="w-3.5 h-3.5 text-green-500"
                      />
                      <Copy
                        v-else
                        class="w-3.5 h-3.5"
                      />
                    </button>
                  </div>

                  <TabsContent value="request-headers">
                    <JsonContent
                      :data="selectedInspectionAttempt.request_headers"
                      view-mode="formatted"
                      :expand-depth="inspectionExpandDepth"
                      :is-dark="isDark"
                      empty-message="无请求头数据"
                    />
                  </TabsContent>

                  <TabsContent value="request-body">
                    <JsonContent
                      :data="selectedInspectionAttempt.request_body"
                      view-mode="formatted"
                      :expand-depth="inspectionExpandDepth"
                      :is-dark="isDark"
                      empty-message="无请求体数据"
                    />
                  </TabsContent>

                  <TabsContent value="response-headers">
                    <JsonContent
                      :data="selectedInspectionAttempt.response_headers"
                      view-mode="formatted"
                      :expand-depth="inspectionExpandDepth"
                      :is-dark="isDark"
                      empty-message="无响应头数据"
                    />
                  </TabsContent>

                  <TabsContent value="response-body">
                    <JsonContent
                      :data="selectedInspectionAttempt.response_body"
                      view-mode="formatted"
                      :expand-depth="inspectionExpandDepth"
                      :is-dark="isDark"
                      empty-message="无响应体数据"
                    />
                  </TabsContent>
                </div>
              </Tabs>
            </div>
          </Card>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="emit('close')"
      >
        {{ showSetup ? '取消' : '关闭' }}
      </Button>
      <Button
        v-if="showResult"
        variant="outline"
        @click="emit('back')"
      >
        返回
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Check, Code2, Copy, Loader2, Maximize2, Minimize2, RotateCcw } from 'lucide-vue-next'
import {
  Badge,
  Card,
  Dialog,
  Tabs,
  TabsContent,
} from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Textarea from '@/components/ui/textarea.vue'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import type { TestAttemptDetail, TestModelFailoverResponse } from '@/api/endpoints/providers'
import type { CandidateRecord, RequestTrace } from '@/api/requestTrace'
import HorizontalRequestTimeline from '@/features/usage/components/HorizontalRequestTimeline.vue'
import JsonContent from '@/features/usage/components/RequestDetailDrawer/JsonContent.vue'
import { useClipboard } from '@/composables/useClipboard'

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
  requestHeadersDraft?: string
  requestHeadersResetValue?: string
  requestHeadersError?: string | null
  requestBodyDraft?: string
  requestBodyResetValue?: string
  requestBodyError?: string | null
  startDisabled?: boolean
}>()

const emit = defineEmits<{
  close: []
  back: []
  start: []
  selectEndpoint: [endpointId: string]
  'update:requestHeadersDraft': [value: string]
  'update:requestBodyDraft': [value: string]
}>()

const endpoints = computed(() => props.endpoints ?? [])
const requestHeadersDraft = computed(() => props.requestHeadersDraft ?? '')
const requestBodyDraft = computed(() => props.requestBodyDraft ?? '')
const traceCandidates = computed(() => props.trace?.candidates ?? [])
const showSetup = computed(() => props.open && !props.testing && !props.result)
const showResult = computed(() => !!props.result)
const showTraceTimeline = computed(() => Boolean(props.requestId) && traceCandidates.value.length > 0)
const isDark = computed(() => typeof document !== 'undefined' && document.documentElement.classList.contains('dark'))
const { copyToClipboard } = useClipboard()

const dialogTitle = computed(() => {
  if (props.result) return '模型测试结果'
  return '模型测试'
})

const dialogDescription = computed(() => {
  if (showSetup.value && props.selectingModelName) {
    return `为 ${props.selectingModelName} 选择端点并编辑测试请求头与请求体`
  }
  if (props.testing && props.selectedEndpoint) {
    return `正在通过 ${formatApiFormat(props.selectedEndpoint.api_format)} 测试 ${props.selectingModelName || '模型'}`
  }
  return ''
})

const resultAttempts = computed(() => props.result?.attempts ?? [])

const hasEffectiveModel = computed(() => {
  if (!props.result) return false
  return resultAttempts.value.some(
    attempt => attempt.effective_model && attempt.effective_model !== props.result?.model,
  )
})

const showEndpointColumn = computed(() => {
  if (!props.result) return false
  if (props.mode === 'direct') return true
  const formats = new Set(resultAttempts.value.map(attempt => attempt.endpoint_api_format))
  return formats.size > 1
})

const resultEmptyMessage = computed(() => {
  if (!props.result) return '没有可用的候选进行测试'
  if (typeof props.result.error === 'string' && props.result.error.trim()) {
    return props.result.error.trim()
  }
  const rawResult = props.result as TestModelFailoverResponse & { message?: string }
  if (typeof rawResult.message === 'string' && rawResult.message.trim()) {
    return rawResult.message.trim()
  }
  return '没有可用的候选进行测试'
})
const showAllAttempts = ref(false)
const inspectionTab = ref<'request-headers' | 'request-body' | 'response-headers' | 'response-body'>('request-body')
const selectedInspectionKey = ref<string | null>(null)
const selectedTraceCandidate = ref<CandidateRecord | null>(null)
const inspectionExpandDepth = ref(0)
const inspectionCopiedStates = ref<Record<string, boolean>>({})

watch(() => props.result, () => {
  showAllAttempts.value = false
  inspectionTab.value = 'request-body'
  inspectionExpandDepth.value = 0
  inspectionCopiedStates.value = {}
  selectedTraceCandidate.value = null
  const defaultAttempt = inspectableAttempts.value[0] ?? resultAttempts.value[0] ?? null
  selectedInspectionKey.value = defaultAttempt ? inspectionKey(defaultAttempt) : null
})

const shouldCollapseAttempts = computed(() => resultAttempts.value.length > 20)

const visibleAttempts = computed(() => {
  if (!shouldCollapseAttempts.value || showAllAttempts.value) {
    return resultAttempts.value
  }
  return resultAttempts.value.slice(0, 20)
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

const inspectableAttempts = computed(() => {
  return resultAttempts.value.filter(hasDebugData)
})

const showDebugInspector = computed(() => {
  return showTraceTimeline.value || inspectableAttempts.value.length > 0
})

const detailTabs = [
  { name: 'request-headers', label: '请求头' },
  { name: 'request-body', label: '请求体' },
  { name: 'response-headers', label: '响应头' },
  { name: 'response-body', label: '响应体' },
] as const

const selectedInspectionAttempt = computed(() => {
  const fromTraceSelection = selectedTraceCandidate.value
    ? findAttemptForTraceCandidate(selectedTraceCandidate.value)
    : null
  if (fromTraceSelection) return fromTraceSelection

  const key = selectedInspectionKey.value
  if (key) {
    const fromKey = resultAttempts.value.find(attempt => inspectionKey(attempt) === key)
    if (fromKey) return fromKey
  }

  return inspectableAttempts.value[0] ?? resultAttempts.value[0] ?? null
})

const timelineRequestApiFormat = computed(() => {
  return props.selectedEndpoint?.api_format || resultAttempts.value[0]?.endpoint_api_format || null
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
  if (status === 'failed' && code >= 200 && code < 300) {
    return `${code} 体内错误`
  }
  return String(code)
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

function inspectionKey(attempt: TestAttemptDetail): string {
  return `${attempt.candidate_index}:${attempt.retry_index ?? 0}:${attempt.key_id}`
}

function findAttemptForTraceCandidate(candidate: CandidateRecord): TestAttemptDetail | null {
  return resultAttempts.value.find(attempt => (
    attempt.candidate_index === candidate.candidate_index
    && (attempt.retry_index ?? 0) === candidate.retry_index
    && (!attempt.key_id || !candidate.key_id || attempt.key_id === candidate.key_id)
  )) ?? null
}

function handleTraceAttemptSelect(candidate: CandidateRecord | null) {
  selectedTraceCandidate.value = candidate
  if (!candidate) return
  const matchedAttempt = findAttemptForTraceCandidate(candidate)
  if (matchedAttempt) {
    selectedInspectionKey.value = inspectionKey(matchedAttempt)
  }
}

function hasDebugData(attempt: TestAttemptDetail): boolean {
  return Boolean(
    attempt.request_url
    || attempt.request_headers
    || attempt.request_body != null
    || attempt.response_headers
    || attempt.response_body != null,
  )
}

function getInspectionTabData(
  tabName: typeof detailTabs[number]['name'],
  attempt: TestAttemptDetail | null,
): unknown {
  if (!attempt) return null
  switch (tabName) {
    case 'request-headers':
      return attempt.request_headers
    case 'request-body':
      return attempt.request_body
    case 'response-headers':
      return attempt.response_headers
    case 'response-body':
      return attempt.response_body
  }
}

function copyInspectionContent(tabName: typeof detailTabs[number]['name']) {
  const data = getInspectionTabData(tabName, selectedInspectionAttempt.value)
  if (data === null || data === undefined || data === '') return

  const text = typeof data === 'string' ? data : JSON.stringify(data, null, 2)
  copyToClipboard(text, false)
  inspectionCopiedStates.value[tabName] = true
  setTimeout(() => {
    inspectionCopiedStates.value[tabName] = false
  }, 2000)
}

function expandInspectionContent() {
  inspectionExpandDepth.value = 999
}

function collapseInspectionContent() {
  inspectionExpandDepth.value = 0
}

function formatRequestHeadersDraft() {
  formatJsonDraft(requestHeadersDraft.value, value => emit('update:requestHeadersDraft', value), '{}')
}

function formatRequestBodyDraft() {
  formatJsonDraft(requestBodyDraft.value, value => emit('update:requestBodyDraft', value))
}

function resetRequestHeadersDraft() {
  emit('update:requestHeadersDraft', props.requestHeadersResetValue ?? '{}')
}

function resetRequestBodyDraft() {
  emit('update:requestBodyDraft', props.requestBodyResetValue ?? '')
}

function formatJsonDraft(
  draft: string,
  onFormatted: (value: string) => void,
  emptyFallback?: string,
) {
  const normalized = draft.trim()
  if (!normalized) {
    if (emptyFallback !== undefined) {
      onFormatted(emptyFallback)
    }
    return
  }

  try {
    const parsed = JSON.parse(normalized)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return
    onFormatted(JSON.stringify(parsed, null, 2))
  } catch {
    // keep user input untouched when JSON is invalid
  }
}
</script>

<style scoped>
.content-block :deep(.rounded-2xl) {
  border: none !important;
  border-radius: 0 !important;
  box-shadow: none !important;
}
</style>
