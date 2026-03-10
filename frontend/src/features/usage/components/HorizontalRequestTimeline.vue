<template>
  <div class="minimal-request-timeline">
    <!-- Loading State -->
    <div
      v-if="loading"
      class="py-4"
    >
      <Skeleton class="h-32 w-full" />
    </div>

    <!-- Error State -->
    <Card
      v-else-if="error"
      class="border-red-200 dark:border-red-800"
    >
      <div class="p-4">
        <p class="text-sm text-red-600 dark:text-red-400">
          {{ error }}
        </p>
      </div>
    </Card>

    <!-- Timeline Content -->
    <div
      v-else-if="trace && trace.candidates.length > 0"
      class="space-y-0"
    >
      <Card>
        <div class="p-6">
          <!-- 概览信息 -->
          <div class="flex items-center justify-between mb-4">
            <div class="flex items-center gap-3">
              <h4 class="text-sm font-semibold">
                请求链路追踪
              </h4>
              <Badge :variant="getFinalStatusBadgeVariant(computedFinalStatus)">
                {{ getFinalStatusLabel(computedFinalStatus) }}
              </Badge>
            </div>
            <div class="text-sm text-muted-foreground">
              {{ formatLatency(totalTraceLatency) }}
            </div>
          </div>

          <!-- 极简时间线轨道（按组显示） -->
          <div class="minimal-track">
            <div
              v-for="(group, groupIndex) in groupedTimeline"
              :key="group.id"
              class="minimal-node-group"
              :class="{
                selected: isGroupSelected(group),
                hovered: isGroupHovered(groupIndex) && !isGroupSelected(group)
              }"
              @mouseenter="hoveredGroupIndex = groupIndex"
              @mouseleave="hoveredGroupIndex = null"
              @click="selectGroup(group)"
            >
              <!-- 节点容器 -->
              <div class="node-container">
                <!-- 节点名称（在节点上方） -->
                <div class="node-label">
                  {{ group.providerName }}
                </div>

                <!-- 主节点（代表首次请求） -->
                <div
                  class="node-dot"
                  :class="[
                    getStatusColorClass(getDisplayStatus(group.primary)),
                    { 'is-first-selected': isGroupSelected(group) && selectedAttemptIndex === 0 }
                  ]"
                  @click.stop="selectFirstAttempt(group)"
                />

                <!-- 子节点（同提供商的其他尝试，不包含首次） -->
                <div
                  v-if="group.retryCount > 0 && isGroupSelected(group)"
                  class="sub-dots"
                >
                  <button
                    v-for="(attempt, idx) in group.allAttempts.slice(1)"
                    :key="attempt.id"
                    class="sub-dot"
                    :class="[
                      getStatusColorClass(getDisplayStatus(attempt)),
                      { active: selectedAttemptIndex === idx + 1 }
                    ]"
                    :title="attempt.key_name || `Key ${idx + 2}`"
                    @click.stop="selectedAttemptIndex = idx + 1"
                  />
                </div>
              </div>

              <!-- 连接线 -->
              <div
                v-if="groupIndex < groupedTimeline.length - 1"
                class="node-line-wrapper"
              >
                <div
                  class="node-line"
                  :class="{ 'conversion-boundary': groupIndex + 1 === conversionBoundaryIndex }"
                />
              </div>
            </div>
          </div>

          <!-- 选中详情面板 -->
          <Transition name="slide-up">
            <div
              v-if="selectedGroup && currentAttempt"
              class="detail-panel"
            >
              <div class="panel-header">
                <div class="panel-title">
                  <span
                    class="title-dot"
                    :class="getStatusColorClass(getDisplayStatus(currentAttempt))"
                  />
                  <span class="title-text">{{ currentGroupTitle }}</span>
                  <a
                    v-if="currentAttempt.provider_website"
                    :href="currentAttempt.provider_website"
                    target="_blank"
                    rel="noopener noreferrer"
                    class="provider-link"
                    @click.stop
                  >
                    <ExternalLink class="w-3 h-3" />
                  </a>
                  <span
                    class="status-tag"
                    :class="getStatusColorClass(getDisplayStatus(currentAttempt))"
                  >
                    {{ currentAttempt.status_code || getStatusLabel(currentAttempt.status) }}
                  </span>
                  <!-- 多 Key 标识 -->
                  <template v-if="selectedGroup.retryCount > 0">
                    <span class="cache-hint">
                      {{ selectedAttemptIndex + 1 }}/{{ selectedGroup.allAttempts.length }}
                    </span>
                  </template>
                </div>
                <div class="panel-nav">
                  <button
                    class="nav-btn"
                    :disabled="selectedGroupIndex === 0"
                    @click.stop="navigateGroup(-1)"
                  >
                    <ChevronLeft class="w-4 h-4" />
                  </button>
                  <span class="nav-info">{{ selectedGroupIndex + 1 }} / {{ groupedTimeline.length }}</span>
                  <button
                    class="nav-btn"
                    :disabled="selectedGroupIndex === groupedTimeline.length - 1"
                    @click.stop="navigateGroup(1)"
                  >
                    <ChevronRight class="w-4 h-4" />
                  </button>
                </div>
              </div>

              <div class="panel-body">
                <!-- 核心信息网格 -->
                <div class="info-grid">
                  <div
                    v-if="currentAttempt.started_at"
                    class="info-item"
                  >
                    <span class="info-label">时间范围</span>
                    <span class="info-value mono time-range-value">
                      {{ formatTime(currentAttempt.started_at) }}
                      <span class="time-arrow-container">
                        <span
                          v-if="currentAttempt.finished_at"
                          class="time-duration"
                        >+{{ formatDuration(currentAttempt.started_at, currentAttempt.finished_at) }}</span>
                        <span class="time-arrow">→</span>
                      </span>
                      {{ currentAttempt.finished_at ? formatTime(currentAttempt.finished_at) : '进行中' }}
                    </span>
                  </div>
                  <div
                    v-if="currentAttempt.extra_data?.first_byte_time_ms != null"
                    class="info-item"
                  >
                    <span class="info-label">首字 (TTFB)</span>
                    <span class="info-value mono">{{ formatLatency(currentAttempt.extra_data.first_byte_time_ms) }}</span>
                  </div>
                  <div
                    v-if="currentAttemptFormatDisplay"
                    class="info-item"
                  >
                    <span class="info-label">格式</span>
                    <span class="info-value">
                      <code class="format-code">{{ currentAttemptFormatDisplay }}</code>
                    </span>
                  </div>
                  <div
                    v-if="currentAttempt.key_name || currentAttempt.key_id"
                    class="info-item"
                  >
                    <span class="info-label">{{ isOAuthType(currentAttempt.key_auth_type) ? '账号' : '密钥' }}</span>
                    <span class="info-value info-value-stacked">
                      <span class="key-name">
                        {{ currentAttempt.key_name || '未知' }}
                        <span
                          v-if="currentAttempt.key_auth_type && currentAttempt.key_auth_type !== 'api_key'"
                          class="auth-type-tag"
                        >{{ formatAuthTypeWithPlan(currentAttempt.key_auth_type, currentAttempt.key_oauth_plan_type) }}</span>
                      </span>
                      <code
                        v-if="currentAttempt.key_preview"
                        class="key-preview"
                      >{{ currentAttempt.key_preview }}</code>
                    </span>
                  </div>
                  <div
                    v-if="currentAttempt.extra_data?.proxy"
                    class="info-item"
                  >
                    <span class="info-label">代理</span>
                    <span class="info-value info-value-stacked">
                      <span class="proxy-name">
                        {{ currentAttempt.extra_data.proxy.node_name || currentAttempt.extra_data.proxy.url || '未知' }}
                        <span
                          v-if="currentAttempt.extra_data.proxy.source === 'system'"
                          class="text-xs text-muted-foreground ml-1"
                        >(系统)</span>
                      </span>
                      <span class="proxy-detail">
                        <span
                          v-if="currentAttempt.extra_data.proxy.ttfb_ms != null"
                          class="text-xs text-muted-foreground"
                        >{{ formatLatency(currentAttempt.extra_data.proxy.ttfb_ms) }}</span>
                        <span
                          v-if="currentAttempt.extra_data.proxy.timing"
                          class="text-xs text-muted-foreground"
                        >(<!--
                          -->{{ proxyTimingBreakdown(currentAttempt.extra_data.proxy) }}<!--
                        -->)</span>
                      </span>
                    </span>
                  </div>
                  <div
                    v-if="currentAttempt.extra_data?.pool_selection"
                    class="info-item"
                  >
                    <span class="info-label">号池调度</span>
                    <span class="info-value info-value-stacked">
                      <span class="pool-reason">
                        <span
                          class="pool-reason-tag"
                          :class="'pool-' + currentAttempt.extra_data.pool_selection.reason"
                        >
                          {{ poolSelectionLabel(currentAttempt.extra_data.pool_selection.reason) }}
                        </span>
                        <span
                          v-if="currentAttempt.extra_data.pool_selection.cost_soft_threshold"
                          class="pool-cost-warn"
                        >接近限额</span>
                      </span>
                      <span
                        v-if="currentAttempt.extra_data.pool_selection.cost_window_usage"
                        class="text-xs text-muted-foreground"
                      >
                        {{ formatNumber(currentAttempt.extra_data.pool_selection.cost_window_usage) }}
                        <template v-if="currentAttempt.extra_data.pool_selection.cost_limit">
                          / {{ formatNumber(currentAttempt.extra_data.pool_selection.cost_limit) }}
                        </template>
                        tokens
                      </span>
                    </span>
                  </div>
                  <div
                    v-if="currentAttempt.extra_data?.pool_skip"
                    class="info-item"
                  >
                    <span class="info-label">号池跳过</span>
                    <span class="info-value info-value-stacked">
                      <span class="pool-skip-type">
                        {{ poolSkipLabel(currentAttempt.extra_data.pool_skip.type) }}
                      </span>
                      <span
                        v-if="currentAttempt.extra_data.pool_skip.cooldown_reason"
                        class="text-xs text-muted-foreground"
                      >
                        {{ currentAttempt.extra_data.pool_skip.cooldown_reason }}
                        <template v-if="currentAttempt.extra_data.pool_skip.cooldown_ttl != null">
                          ({{ currentAttempt.extra_data.pool_skip.cooldown_ttl }}s)
                        </template>
                      </span>
                      <span
                        v-if="currentAttempt.extra_data.pool_skip.cost_window_usage"
                        class="text-xs text-muted-foreground"
                      >
                        {{ formatNumber(currentAttempt.extra_data.pool_skip.cost_window_usage) }} tokens
                      </span>
                    </span>
                  </div>
                  <div
                    v-if="mergedCapabilities.length > 0"
                    class="info-item"
                  >
                    <span class="info-label">能力</span>
                    <span class="info-value">
                      <span class="capability-tags">
                        <span
                          v-for="cap in mergedCapabilities"
                          :key="cap"
                          class="capability-tag"
                          :class="{ active: isCapabilityUsed(cap) }"
                        >{{ formatCapabilityLabel(cap) }}</span>
                      </span>
                    </span>
                  </div>
                </div>

                <!-- 用量与费用（仅成功节点显示） -->
                <div
                  v-if="currentAttempt.status === 'success' && usageData"
                  class="usage-section"
                >
                  <div class="usage-grid">
                    <!-- 输入 输出 -->
                    <div class="usage-row">
                      <div class="usage-item">
                        <span class="usage-label">输入</span>
                        <span class="usage-tokens">{{ formatNumber(usageData.tokens.input) }}</span>
                        <span class="usage-cost">${{ usageData.cost.input.toFixed(6) }}</span>
                      </div>
                      <div class="usage-divider" />
                      <div class="usage-item">
                        <span class="usage-label">输出</span>
                        <span class="usage-tokens">{{ formatNumber(usageData.tokens.output) }}</span>
                        <span class="usage-cost">${{ usageData.cost.output.toFixed(6) }}</span>
                      </div>
                    </div>
                    <!-- 缓存创建 缓存读取（仅在有缓存数据时显示） -->
                    <div
                      v-if="usageData.tokens.cache_creation || usageData.tokens.cache_read"
                      class="usage-row"
                    >
                      <div class="usage-item">
                        <span class="usage-label">缓存创建</span>
                        <span class="usage-tokens">{{ formatNumber(usageData.tokens.cache_creation || 0) }}</span>
                        <span class="usage-cost">${{ (usageData.cost.cache_creation || 0).toFixed(6) }}</span>
                      </div>
                      <div class="usage-divider" />
                      <div class="usage-item">
                        <span class="usage-label">缓存读取</span>
                        <span class="usage-tokens">{{ formatNumber(usageData.tokens.cache_read || 0) }}</span>
                        <span class="usage-cost">${{ (usageData.cost.cache_read || 0).toFixed(6) }}</span>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- 跳过原因 -->
                <div
                  v-if="currentAttempt.skip_reason"
                  class="skip-reason"
                >
                  <span class="reason-label">跳过原因</span>
                  <span class="reason-value">{{ currentAttempt.skip_reason }}</span>
                </div>

                <!-- 错误信息 -->
                <div
                  v-if="currentAttempt.status === 'failed' && (currentAttempt.error_message || currentAttempt.error_type)"
                  class="error-block"
                >
                  <div class="error-type">
                    {{ currentAttempt.error_type || '错误' }}
                  </div>
                  <div class="error-msg">
                    {{ currentAttempt.error_message || '未知错误' }}
                  </div>
                </div>

                <!-- 额外数据 -->
                <details
                  v-if="currentAttempt.extra_data && Object.keys(currentAttempt.extra_data).length > 0"
                  class="extra-block"
                >
                  <summary class="extra-toggle">
                    额外信息
                  </summary>
                  <pre class="extra-json">{{ JSON.stringify(currentAttempt.extra_data, null, 2) }}</pre>
                </details>
              </div>
            </div>
          </Transition>
        </div>
      </Card>
    </div>

    <!-- Empty State -->
    <Card
      v-else
      class="border-dashed"
    >
      <div class="p-8 text-center">
        <p class="text-sm text-muted-foreground">
          暂无追踪数据
        </p>
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Skeleton from '@/components/ui/skeleton.vue'
import { ChevronLeft, ChevronRight, ExternalLink } from 'lucide-vue-next'
import { requestTraceApi, type RequestTrace, type CandidateRecord } from '@/api/requestTrace'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'
import { formatApiFormat } from '@/api/endpoints/types/api-format'

// 节点组类型
interface NodeGroup {
  id: string
  providerName: string
  primary: CandidateRecord
  primaryStatus: string
  allAttempts: CandidateRecord[]  // 所有尝试（包括首次和重试）
  retryCount: number
  totalLatency: number  // 所有尝试的总延迟
  startIndex: number
  endIndex: number
  hasConversion: boolean  // 组内是否有格式转换候选
  providerApiFormat: string | null  // 提供商 API 格式（如 openai:cli）
  isPoolGroup?: boolean
}

// 用量数据类型
interface UsageData {
  tokens: {
    input: number
    output: number
    cache_creation: number
    cache_read: number
  }
  cost: {
    input: number
    output: number
    cache_creation: number
    cache_read: number
    per_request: number
    total: number
  }
  pricing: {
    input?: number
    output?: number
    cache_creation?: number
    cache_read?: number
    per_request?: number
  }
}

const props = defineProps<{
  requestId: string
  /** 外部传入的状态码，用于覆盖 trace.final_status 的判断 */
  overrideStatusCode?: number
  /** 请求侧 API 格式（客户端入口格式） */
  requestApiFormat?: string | null
  /** 用量和费用数据 */
  usageData?: UsageData | null
  /** 请求元数据（用于号池调度组装） */
  requestMetadata?: Record<string, unknown> | null
}>()

// 用量数据（从 props 获取）
const usageData = computed(() => props.usageData)

// 格式化数字
const formatNumber = (num: number): string => {
  return num.toLocaleString('zh-CN')
}

// 计算最终状态：优先检查进行中状态，再使用外部状态码
const computedFinalStatus = computed(() => {
  // 优先检查是否有进行中或流式传输的候选（请求尚未完成）
  const hasPending = trace.value?.candidates?.some(
    c => c.status === 'pending' || c.status === 'streaming'
  )
  if (hasPending) {
    return 'pending'
  }

  // 使用外部状态码判断最终状态
  if (props.overrideStatusCode !== undefined) {
    return props.overrideStatusCode === 200 ? 'success' : 'failed'
  }

  return trace.value?.final_status || 'pending'
})

// 获取最终状态标签
const getFinalStatusLabel = (status: string) => {
  const labels: Record<string, string> = {
    success: '最终成功',
    failed: '最终失败',
    streaming: '流式传输中',
    pending: '进行中'
  }
  return labels[status] || status
}

// 获取最终状态徽章样式
type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark'

const getFinalStatusBadgeVariant = (status: string): BadgeVariant => {
  const variants: Record<string, BadgeVariant> = {
    success: 'success',
    failed: 'destructive',
    streaming: 'secondary',
    pending: 'secondary'
  }
  return variants[status] || 'default'
}

const loading = ref(false)
const error = ref<string | null>(null)
const trace = ref<RequestTrace | null>(null)
const selectedGroupIndex = ref(0)
const selectedAttemptIndex = ref(0)
const hoveredGroupIndex = ref<number | null>(null)

// 格式化延迟（自动调整单位）
const formatLatency = (ms: number | undefined | null): string => {
  if (ms === undefined || ms === null) return '-'
  if (ms >= 1000) {
    return `${(ms / 1000).toFixed(2)}s`
  }
  return `${ms}ms`
}

// 格式化字节大小
const formatSize = (bytes: number): string => {
  if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(1)}MB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)}KB`
  return `${bytes}B`
}

// 代理 timing 分阶段展示
const proxyTimingBreakdown = (proxy: Record<string, unknown>): string => {
  const t = proxy.timing as Record<string, number | null | undefined> | undefined
  if (!t) return ''

  const parts: string[] = []

  // 兼容旧版 timing（含 body_read_ms/decompress_ms）
  const readDecompress = ((t.body_read_ms as number) || 0) + ((t.decompress_ms as number) || 0)
  if (readDecompress > 0) {
    let label = `读取 ${formatLatency(readDecompress)}`
    if (t.decompress_ms != null && t.decompress_ms > 0 && t.wire_size != null && t.body_size != null && (t.body_size as number) > 0) {
      const ratio = Math.round((1 - (t.wire_size as number) / (t.body_size as number)) * 100)
      label += ` ${formatSize(t.wire_size as number)}→${formatSize(t.body_size as number)}`
      if (ratio > 0) label += ` -${ratio}%`
    }
    parts.push(label)
  }

  const ttfbMs = t.ttfb_ms ?? t.upstream_ms
  const responseWaitMs = t.response_wait_ms ?? (
    t.connection_acquire_ms != null && ttfbMs != null
      ? Math.max(0, (ttfbMs as number) - (t.connection_acquire_ms as number))
      : null
  )
  const legacyWaitMs = t.upstream_processing_ms ?? (
    ttfbMs != null && t.connect_ms != null && t.tls_ms != null
      ? Math.max(0, (ttfbMs as number) - (t.connect_ms as number) - (t.tls_ms as number))
      : null
  )

  if (t.dns_ms != null && (t.dns_ms as number) > 0) {
    parts.push(`DNS ${formatLatency(t.dns_ms as number)}`)
  }
  if (t.connection_reused === true) {
    parts.push('复用连接')
  }
  if (t.connect_ms != null && (t.connect_ms as number) > 0) {
    parts.push(`连接 ${formatLatency(t.connect_ms as number)}`)
  }
  if (t.tls_ms != null && (t.tls_ms as number) > 0) {
    parts.push(`TLS ${formatLatency(t.tls_ms as number)}`)
  }
  if (ttfbMs != null && (ttfbMs as number) > 0) {
    parts.push(`TTFB ${formatLatency(ttfbMs as number)}`)
  }
  if (responseWaitMs != null && (responseWaitMs as number) > 0) {
    parts.push(`等待响应头 ${formatLatency(Math.round(responseWaitMs as number))}`)
  } else if (legacyWaitMs != null && (legacyWaitMs as number) > 0) {
    parts.push(`等待响应头(旧版估算) ${formatLatency(Math.round(legacyWaitMs as number))}`)
  }

  // 计算 Aether→代理 之间无法解释的耗时差
  if (proxy.ttfb_ms != null && t.total_ms != null) {
    const gap = (proxy.ttfb_ms as number) - (t.total_ms as number)
    if (gap > 500) {
      parts.push(`传输 ${formatLatency(Math.round(gap))}`)
    }
  }

  return parts.join(' / ')
}

const TIMELINE_STATUS: CandidateRecord['status'][] = [
  'success',
  'failed',
  'skipped',
  'cancelled',
  'pending',
  'streaming',
  'available',
  'unused',
  'stream_interrupted',
]

const STATUS_PRIORITY: Record<string, number> = {
  available: 0,
  unused: 0,
  skipped: 1,
  failed: 2,
  cancelled: 2,
  stream_interrupted: 2,
  pending: 3,
  streaming: 3,
  success: 4,
}

const toInt = (value: unknown, defaultValue = 0): number => {
  const num = Number(value)
  return Number.isFinite(num) ? Math.trunc(num) : defaultValue
}

const makeAttemptKey = (candidateIndex: number, retryIndex: number): string => {
  return `${candidateIndex}:${retryIndex}`
}

const POOL_UNATTEMPTED_STATUS = new Set<CandidateRecord['status']>([
  'available',
  'unused',
  'skipped',
])

const isPoolAttemptedCandidate = (candidate: CandidateRecord): boolean => {
  if (POOL_UNATTEMPTED_STATUS.has(candidate.status)) return false
  // pending 只有开始执行后才算真正进入号池内部尝试
  if (candidate.status === 'pending' && !candidate.started_at) return false
  return true
}

const normalizeTimelineStatus = (value: unknown): CandidateRecord['status'] => {
  if (typeof value !== 'string') return 'failed'
  const normalized = value.trim().toLowerCase()
  if ((TIMELINE_STATUS as string[]).includes(normalized)) {
    return normalized as CandidateRecord['status']
  }
  // 兜底：内部调度轨迹里非标准状态统一按失败展示
  return 'failed'
}

const extractPoolGroupId = (candidate: CandidateRecord): string | null => {
  const extra = candidate.extra_data
  if (!extra || typeof extra !== 'object' || Array.isArray(extra)) return null
  const value = (extra as Record<string, unknown>).pool_group_id
  if (typeof value !== 'string') return null
  const text = value.trim()
  return text || null
}

// 候选时间线（按实际执行顺序排序）
const rawTimeline = computed<CandidateRecord[]>(() => {
  if (!trace.value) return []
  return [...trace.value.candidates]
    .filter(c => TIMELINE_STATUS.includes(c.status))
    .sort((a, b) => {
      const startedA = a.started_at ? new Date(a.started_at).getTime() : Infinity
      const startedB = b.started_at ? new Date(b.started_at).getTime() : Infinity
      if (startedA !== startedB) return startedA - startedB
      if (a.candidate_index !== b.candidate_index) {
        return a.candidate_index - b.candidate_index
      }
      return a.retry_index - b.retry_index
    })
})


const schedulingAudit = computed<Record<string, unknown> | null>(() => {
  const metadata = props.requestMetadata
  if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) return null
  const raw = metadata.scheduling_audit
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  return raw as Record<string, unknown>
})

const poolAttemptCandidates = computed<CandidateRecord[]>(() => {
  // 新链路：优先使用后端写入的 extra_data.pool_group_id，
  // 但仅展示实际进入号池执行的 key（排除 available/unused/skipped）。
  const fromTrace = rawTimeline.value.filter(
    (candidate) => extractPoolGroupId(candidate) !== null && isPoolAttemptedCandidate(candidate),
  )
  if (fromTrace.length > 0) {
    return fromTrace
  }

  // 兼容旧链路：回退到 request_metadata.scheduling_audit.attempts。
  const audit = schedulingAudit.value
  if (!audit) return []
  const attempts = audit.attempts
  if (!Array.isArray(attempts) || attempts.length === 0) return []

  const providerNameById = new Map<string, string>()
  for (const candidate of rawTimeline.value) {
    const providerId = String(candidate.provider_id || '').trim()
    const providerName = String(candidate.provider_name || '').trim()
    if (!providerId || !providerName) continue
    if (!providerNameById.has(providerId)) {
      providerNameById.set(providerId, providerName)
    }
  }
  const providerTypeLikeNames = new Set<string>([
    'codex',
    'kiro',
    'antigravity',
    'claude_code',
    'claude code',
    'gemini_cli',
    'gemini cli',
    'oauth',
    'api_key',
    'api key',
  ])

  const traceMap = new Map<string, CandidateRecord>()
  for (const candidate of rawTimeline.value) {
    traceMap.set(makeAttemptKey(candidate.candidate_index, candidate.retry_index), candidate)
  }

  return attempts
    .map((item, index) => {
      if (!item || typeof item !== 'object' || Array.isArray(item)) return null
      const raw = item as Record<string, unknown>
      const candidateIndex = toInt(raw.candidate_index, index)
      const retryIndex = toInt(raw.retry_index, 0)
      const key = makeAttemptKey(candidateIndex, retryIndex)
      const fromTrace = traceMap.get(key)

      const merged: CandidateRecord = fromTrace
        ? { ...fromTrace }
        : {
            id: `pool-${props.requestId}-${candidateIndex}-${retryIndex}-${index}`,
            request_id: props.requestId,
            candidate_index: candidateIndex,
            retry_index: retryIndex,
            provider_id: undefined,
            provider_name: undefined,
            endpoint_id: undefined,
            key_id: undefined,
            key_name: undefined,
            status: 'failed',
            is_cached: false,
            created_at: new Date(0).toISOString(),
          }

      merged.status = normalizeTimelineStatus(raw.status ?? merged.status)
      if (typeof raw.provider_id === 'string') merged.provider_id = raw.provider_id
      if (typeof raw.provider_name === 'string') merged.provider_name = raw.provider_name
      if (typeof raw.endpoint_id === 'string') merged.endpoint_id = raw.endpoint_id
      if (typeof raw.key_id === 'string') merged.key_id = raw.key_id
      if (typeof raw.key_name === 'string') merged.key_name = raw.key_name
      if (typeof raw.status_code === 'number') merged.status_code = raw.status_code
      if (typeof raw.error_type === 'string') merged.error_type = raw.error_type
      const rawPoolGroupId = typeof raw.pool_group_id === 'string' ? raw.pool_group_id.trim() : ''
      const fallbackPoolGroupId = typeof raw.provider_id === 'string' ? raw.provider_id.trim() : ''
      const finalPoolGroupId = rawPoolGroupId || fallbackPoolGroupId
      if (finalPoolGroupId) {
        merged.extra_data = {
          ...(merged.extra_data || {}),
          pool_group_id: finalPoolGroupId,
        }
      }

      const mergedProviderId = String(merged.provider_id || '').trim()
      if (mergedProviderId) {
        const inferredProviderName = providerNameById.get(mergedProviderId)
        const currentProviderName = String(merged.provider_name || '').trim()
        if (
          inferredProviderName
          && (
            !currentProviderName
            || providerTypeLikeNames.has(currentProviderName.toLowerCase())
          )
        ) {
          merged.provider_name = inferredProviderName
        }
      }
      return merged
    })
    .filter((item): item is CandidateRecord => item !== null)
})

const poolAttemptsByGroup = computed<Map<string, CandidateRecord[]>>(() => {
  const grouped = new Map<string, CandidateRecord[]>()
  for (const attempt of poolAttemptCandidates.value) {
    const groupId =
      extractPoolGroupId(attempt)
      || String(attempt.provider_id || '').trim()
      || '__pool_group__'
    const existing = grouped.get(groupId)
    if (existing) {
      existing.push(attempt)
    } else {
      grouped.set(groupId, [attempt])
    }
  }
  return grouped
})

const poolAttemptKeySet = computed<Set<string>>(() => {
  return new Set(
    poolAttemptCandidates.value.map((item) => makeAttemptKey(item.candidate_index, item.retry_index)),
  )
})

const timeline = computed<CandidateRecord[]>(() => {
  if (poolAttemptCandidates.value.length === 0) return rawTimeline.value
  return rawTimeline.value.filter(
    (candidate) => !poolAttemptKeySet.value.has(makeAttemptKey(candidate.candidate_index, candidate.retry_index)),
  )
})

const AUTH_TYPE_PROVIDER_LABEL_MAP: Record<string, string> = {
  codex: 'Codex',
  kiro: 'Kiro',
  antigravity: 'Antigravity',
  claude_code: 'Claude Code',
  gemini_cli: 'Gemini CLI',
}

const getProviderDisplayName = (
  attempt: CandidateRecord | null | undefined,
  options: { allowAuthTypeFallback?: boolean } = {},
): string => {
  const allowAuthTypeFallback = options.allowAuthTypeFallback ?? true
  if (!attempt) return '未知'
  const providerName = String(attempt.provider_name || '').trim()
  if (providerName) return providerName
  if (allowAuthTypeFallback) {
    const authType = String(attempt.key_auth_type || '').trim().toLowerCase()
    if (authType && AUTH_TYPE_PROVIDER_LABEL_MAP[authType]) {
      return AUTH_TYPE_PROVIDER_LABEL_MAP[authType]
    }
  }
  return '未知'
}

const normalizeProviderIdentity = (value: unknown): string => {
  if (typeof value !== 'string') return ''
  return value.trim().toLowerCase()
}

const buildProviderGroups = (items: CandidateRecord[]): NodeGroup[] => {
  const groups: NodeGroup[] = []
  let currentGroup: NodeGroup | null = null

  items.forEach((candidate) => {
    const providerKey = candidate.provider_name || '未知'

    if (currentGroup && currentGroup.id === providerKey) {
      currentGroup.allAttempts.push(candidate)
      currentGroup.retryCount++
      currentGroup.endIndex = candidate.candidate_index
      currentGroup.totalLatency += candidate.latency_ms || 0
      if (candidate.extra_data?.needs_conversion) {
        currentGroup.hasConversion = true
      }
      const currentPriority = STATUS_PRIORITY[currentGroup.primaryStatus] ?? 0
      const newPriority = STATUS_PRIORITY[candidate.status] ?? 0
      if (newPriority > currentPriority) {
        currentGroup.primaryStatus = candidate.status
      }
      return
    }

    currentGroup = {
      id: providerKey,
      providerName: getProviderDisplayName(candidate),
      primary: candidate,
      primaryStatus: candidate.status,
      allAttempts: [candidate],
      retryCount: 0,
      totalLatency: candidate.latency_ms || 0,
      startIndex: candidate.candidate_index,
      endIndex: candidate.candidate_index,
      hasConversion: candidate.extra_data?.needs_conversion === true,
      providerApiFormat: candidate.extra_data?.provider_api_format || null,
      isPoolGroup: false,
    }
    groups.push(currentGroup)
  })

  return groups
}

// 将相同 Provider 的所有请求合并为组（同提供商的 Key 放在子节点）
const groupedTimeline = computed<NodeGroup[]>(() => {
  const providerGroups = buildProviderGroups(timeline.value)
  if (poolAttemptsByGroup.value.size === 0) {
    return providerGroups
  }

  const poolProviderIds = new Set<string>()
  const poolProviderNames = new Set<string>()
  const poolGroups: NodeGroup[] = []

  for (const [groupId, attemptsRaw] of poolAttemptsByGroup.value.entries()) {
    const attempts = [...attemptsRaw].sort((a, b) => {
      if (a.candidate_index !== b.candidate_index) {
        return a.candidate_index - b.candidate_index
      }
      return a.retry_index - b.retry_index
    })
    if (attempts.length === 0) continue

    const poolPrimaryStatus = attempts.reduce((best, current) => {
      const bestPriority = STATUS_PRIORITY[best] ?? 0
      const currentPriority = STATUS_PRIORITY[current.status] ?? 0
      return currentPriority > bestPriority ? current.status : best
    }, attempts[0].status)

    const successAttempt = attempts.find((item) => item.status === 'success')
    const poolPrimary = successAttempt || attempts[attempts.length - 1] || attempts[0]
    const startIndex = Math.min(...attempts.map(item => item.candidate_index))
    const endIndex = Math.max(...attempts.map(item => item.candidate_index))

    poolGroups.push({
      id: `pool:${groupId}`,
      providerName: getProviderDisplayName(poolPrimary, { allowAuthTypeFallback: false }),
      primary: poolPrimary,
      primaryStatus: poolPrimaryStatus,
      allAttempts: attempts,
      retryCount: Math.max(0, attempts.length - 1),
      totalLatency: attempts.reduce((sum, item) => sum + (item.latency_ms || 0), 0),
      startIndex,
      endIndex,
      hasConversion: attempts.some((item) => item.extra_data?.needs_conversion === true),
      providerApiFormat: null,
      isPoolGroup: true,
    })

    for (const attempt of attempts) {
      const providerId = String(attempt.provider_id || '').trim()
      if (providerId) poolProviderIds.add(providerId)
      const providerName = normalizeProviderIdentity(attempt.provider_name)
      if (providerName) poolProviderNames.add(providerName)
    }
  }

  const dedupedProviderGroups = providerGroups.filter((group) => {
    const sameProviderById = group.allAttempts.some((attempt) => {
      const providerId = String(attempt.provider_id || '').trim()
      return providerId !== '' && poolProviderIds.has(providerId)
    })
    if (sameProviderById) return false

    const groupName = normalizeProviderIdentity(group.primary.provider_name || group.providerName)
    if (groupName && poolProviderNames.has(groupName)) return false

    return true
  })

  const allGroups = [...poolGroups, ...dedupedProviderGroups]
  allGroups.sort((a, b) => a.startIndex - b.startIndex)
  return allGroups
})

// 格式转换分界点索引（首个 hasConversion=true 的 group index）
const conversionBoundaryIndex = computed(() => {
  const groups = groupedTimeline.value
  if (!groups || groups.length === 0) return -1
  const idx = groups.findIndex(g => g.hasConversion)
  // 只有当分界点不在最开头时才有意义（前面有 exact 候选）
  if (idx <= 0) return -1
  return idx
})

// 计算链路总耗时（使用成功候选的 latency_ms 字段）
// 优先使用 latency_ms，因为它与 Usage.response_time_ms 使用相同的时间基准
// 避免 finished_at - started_at 带来的额外延迟（数据库操作时间）
const totalTraceLatency = computed(() => {
  if (!rawTimeline.value || rawTimeline.value.length === 0) return 0

  // 查找成功的候选，使用其 latency_ms
  const successCandidate = rawTimeline.value.find(c => c.status === 'success')
  if (successCandidate?.latency_ms != null) {
    return successCandidate.latency_ms
  }

  // 如果没有成功的候选，查找失败但有 latency_ms 的候选
  const failedWithLatency = rawTimeline.value.find(c => c.status === 'failed' && c.latency_ms != null)
  if (failedWithLatency?.latency_ms != null) {
    return failedWithLatency.latency_ms
  }

  // 回退：使用 finished_at - started_at 计算
  let earliestStart: number | null = null
  let latestEnd: number | null = null

  for (const candidate of rawTimeline.value) {
    if (candidate.started_at) {
      const startTime = new Date(candidate.started_at).getTime()
      if (earliestStart === null || startTime < earliestStart) {
        earliestStart = startTime
      }
    }
    if (candidate.finished_at) {
      const endTime = new Date(candidate.finished_at).getTime()
      if (latestEnd === null || endTime > latestEnd) {
        latestEnd = endTime
      }
    }
  }

  if (earliestStart !== null && latestEnd !== null) {
    return latestEnd - earliestStart
  }
  return 0
})

// 计算选中的组
const selectedGroup = computed(() => {
  if (!groupedTimeline.value || groupedTimeline.value.length === 0) return null
  return groupedTimeline.value[selectedGroupIndex.value]
})

// 计算当前查看的尝试
const currentAttempt = computed(() => {
  if (!selectedGroup.value) return null
  return selectedGroup.value.allAttempts[selectedAttemptIndex.value] || selectedGroup.value.primary
})

const currentGroupTitle = computed(() => {
  if (!selectedGroup.value || !currentAttempt.value) return ''
  if (selectedGroup.value.isPoolGroup) {
    return getProviderDisplayName(currentAttempt.value)
  }
  return selectedGroup.value.providerName
})

const normalizeFormatSignature = (value: string): string => {
  return value.trim().toLowerCase()
}

const currentAttemptFormatDisplay = computed(() => {
  const attempt = currentAttempt.value
  if (!attempt) return ''
  const extra = (
    attempt.extra_data && typeof attempt.extra_data === 'object' && !Array.isArray(attempt.extra_data)
      ? attempt.extra_data
      : {}
  ) as Record<string, unknown>

  const providerRaw = typeof extra.provider_api_format === 'string' ? extra.provider_api_format : ''
  const clientRawFromExtra = typeof extra.client_api_format === 'string' ? extra.client_api_format : ''
  const requestRaw = clientRawFromExtra || (typeof props.requestApiFormat === 'string' ? props.requestApiFormat : '')

  if (!providerRaw && !requestRaw) return ''

  const providerText = providerRaw ? formatApiFormat(providerRaw) : ''
  const requestText = requestRaw ? formatApiFormat(requestRaw) : ''
  const convertedByFlag = extra.needs_conversion === true
  const convertedByDiff = Boolean(
    providerRaw &&
      requestRaw &&
      normalizeFormatSignature(providerRaw) !== normalizeFormatSignature(requestRaw),
  )

  if ((convertedByFlag || convertedByDiff) && requestText && providerText) {
    return `${requestText} -> ${providerText}`
  }

  return providerText || requestText
})

// 计算当前尝试启用的能力标签（请求需要的能力）
const activeCapabilities = computed(() => {
  if (!currentAttempt.value?.required_capabilities) return []
  const caps = currentAttempt.value.required_capabilities
  // 只返回值为 true 的能力
  return Object.entries(caps)
    .filter(([_, enabled]) => enabled)
    .map(([key]) => key)
})

// 计算当前 Key 支持的能力标签
const keyCapabilities = computed(() => {
  if (!currentAttempt.value?.key_capabilities) return []
  const caps = currentAttempt.value.key_capabilities
  // 只返回值为 true 的能力
  return Object.entries(caps)
    .filter(([_, enabled]) => enabled)
    .map(([key]) => key)
})

// 合并后的能力列表：Key 支持的能力 + 请求需要的能力（去重）
const mergedCapabilities = computed(() => {
  const keyCaps = new Set(keyCapabilities.value)
  const activeCaps = new Set(activeCapabilities.value)
  // 合并两个集合
  const merged = new Set([...keyCaps, ...activeCaps])
  return Array.from(merged)
})

// 检查某个能力是否被请求使用
const isCapabilityUsed = (cap: string): boolean => {
  return activeCapabilities.value.includes(cap)
}

// 判断是否为 OAuth 类型（provider_type 为具体值时也算 OAuth）
const isOAuthType = (authType?: string): boolean => {
  if (!authType) return false
  return !['api_key', 'service_account'].includes(authType)
}

// 格式化认证类型（合并 plan 信息，避免冗余）
const formatAuthTypeWithPlan = (authType: string, planType?: string): string => {
  const labels: Record<string, string> = {
    'oauth': 'OAuth',
    'service_account': 'Service Account',
    'kiro': 'Kiro',
    'codex': 'Codex',
    'antigravity': 'Antigravity',
    'claude_code': 'Claude Code',
    'gemini_cli': 'Gemini CLI',
  }
  const typeName = labels[authType] || authType
  if (planType) {
    return `${typeName} ${planType}`
  }
  return typeName
}

// 格式化能力标签显示
const formatCapabilityLabel = (cap: string): string => {
  const labels: Record<string, string> = {
    'cache_1h': '1h缓存',
    'cache_5min': '5min缓存',
    'context_1m': '1M上下文',
    'context_200k': '200K上下文',
    'extended_thinking': '深度思考',
    'vision': '视觉',
    'function_calling': '函数调用',
  }
  return labels[cap] || cap
}

const poolSelectionLabel = (reason: string): string => {
  const labels: Record<string, string> = {
    sticky: '粘性会话',
    lru: 'LRU',
    random: '随机',
    tiebreak: '随机 (平分)',
  }
  return labels[reason] || reason
}

const poolSkipLabel = (type: string): string => {
  const labels: Record<string, string> = {
    cooldown: '冷却中',
    cost_exhausted: '额度耗尽',
    upstream: '上游跳过',
  }
  return labels[type] || type
}

// 检查组是否被悬浮
const isGroupHovered = (groupIndex: number) => {
  return hoveredGroupIndex.value === groupIndex
}

// 检查组是否被选中
const isGroupSelected = (group: NodeGroup) => {
  return selectedGroupIndex.value === groupedTimeline.value.findIndex(g => g.id === group.id && g.startIndex === group.startIndex)
}

// 选中一个组
const selectGroup = (group: NodeGroup) => {
  const index = groupedTimeline.value.findIndex(g => g.id === group.id && g.startIndex === group.startIndex)
  if (index >= 0) {
    selectedGroupIndex.value = index
    // 默认选中成功的尝试，或最后一个尝试
    const successIdx = group.allAttempts.findIndex(a => a.status === 'success')
    selectedAttemptIndex.value = successIdx >= 0 ? successIdx : group.allAttempts.length - 1
  }
}

// 选中一个组的首次请求
const selectFirstAttempt = (group: NodeGroup) => {
  const index = groupedTimeline.value.findIndex(g => g.id === group.id && g.startIndex === group.startIndex)
  if (index >= 0) {
    selectedGroupIndex.value = index
    selectedAttemptIndex.value = 0
  }
}

// 导航到上/下一组
const navigateGroup = (direction: number) => {
  const newIndex = selectedGroupIndex.value + direction
  if (newIndex >= 0 && newIndex < groupedTimeline.value.length) {
    selectedGroupIndex.value = newIndex
    const group = groupedTimeline.value[newIndex]
    // 默认选中成功的尝试，或最后一个尝试
    const successIdx = group.allAttempts.findIndex(a => a.status === 'success')
    selectedAttemptIndex.value = successIdx >= 0 ? successIdx : group.allAttempts.length - 1
  }
}

// 加载请求追踪数据
const isSilentRefresh = ref(false)
const loadTrace = async (silent = false) => {
  if (!props.requestId) return

  isSilentRefresh.value = silent

  if (!silent) {
    loading.value = true
  }
  error.value = null

  try {
    trace.value = await requestTraceApi.getRequestTrace(props.requestId)
  } catch (err: unknown) {
    if (!silent) {
      error.value = parseApiError(err, '加载失败')
    }
    log.error('加载请求追踪失败:', err)
  } finally {
    if (!silent) {
      loading.value = false
    }
  }
}

// 监听 groupedTimeline 变化，自动选择最有意义的组
watch(groupedTimeline, (newGroups) => {
  if (!newGroups || newGroups.length === 0) return

  // 静默刷新时不重置选中状态
  if (isSilentRefresh.value) {
    isSilentRefresh.value = false
    return
  }

  // 查找成功的组
  const successIdx = newGroups.findIndex(g => g.primaryStatus === 'success')
  if (successIdx >= 0) {
    selectedGroupIndex.value = successIdx
    // 选中成功的尝试
    const group = newGroups[successIdx]
    const attemptIdx = group.allAttempts.findIndex(a => a.status === 'success')
    selectedAttemptIndex.value = attemptIdx >= 0 ? attemptIdx : 0
    return
  }

  // 查找正在进行的组
  const activeIdx = newGroups.findIndex(g => g.primaryStatus === 'pending' || g.primaryStatus === 'streaming')
  if (activeIdx >= 0) {
    selectedGroupIndex.value = activeIdx
    // 选中正在进行的尝试，而非最后一个
    const group = newGroups[activeIdx]
    const attemptIdx = group.allAttempts.findIndex(a => a.status === 'pending' || a.status === 'streaming')
    selectedAttemptIndex.value = attemptIdx >= 0 ? attemptIdx : group.allAttempts.length - 1
    return
  }

  // 查找最后一个有效结果的组（有实际执行过的状态：failed/cancelled/stream_interrupted/skipped）
  // 从后往前找第一个有效状态的组
  const activeStatuses = ['failed', 'cancelled', 'stream_interrupted', 'skipped']
  for (let i = newGroups.length - 1; i >= 0; i--) {
    const group = newGroups[i]
    if (activeStatuses.includes(group.primaryStatus)) {
      selectedGroupIndex.value = i
      // 选中最后一个有效状态的尝试（从后往前遍历）
      let targetIdx = -1
      for (let j = group.allAttempts.length - 1; j >= 0; j--) {
        if (activeStatuses.includes(group.allAttempts[j].status)) {
          targetIdx = j
          break
        }
      }
      selectedAttemptIndex.value = targetIdx >= 0 ? targetIdx : group.allAttempts.length - 1
      return
    }
  }

  // 都没有有效状态，选择第一个组（避免选到末尾的未执行节点）
  selectedGroupIndex.value = 0
  selectedAttemptIndex.value = 0
}, { immediate: true })

// 监听 requestId 变化
watch(() => props.requestId, () => {
  selectedGroupIndex.value = 0
  selectedAttemptIndex.value = 0
  loadTrace()
}, { immediate: true })

defineExpose({ refresh: () => loadTrace(true) })

// 格式化时间（详细）
const formatTime = (dateStr: string) => {
  const date = new Date(dateStr)
  const timeStr = date.toLocaleTimeString('zh-CN', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
  const ms = date.getMilliseconds().toString().padStart(3, '0')
  return `${timeStr}.${ms}`
}

// 格式化持续时间（开始到结束）
const formatDuration = (startStr: string, endStr: string): string => {
  const start = new Date(startStr).getTime()
  const end = new Date(endStr).getTime()
  const durationMs = end - start
  if (durationMs >= 1000) {
    return `${(durationMs / 1000).toFixed(2)}s`
  }
  return `${durationMs}ms`
}

// 获取状态标签
const getStatusLabel = (status: string) => {
  const labels: Record<string, string> = {
    available: '未执行',
    unused: '未执行',
    pending: '等待中',
    streaming: '传输中',
    stream_interrupted: '流中断',
    success: '成功',
    failed: '失败',
    cancelled: '已取消',
    skipped: '跳过'
  }
  return labels[status] || status
}

// 获取状态颜色类
const getStatusColorClass = (status: string) => {
  const classes: Record<string, string> = {
    available: 'status-available',
    unused: 'status-available',
    pending: 'status-pending',
    streaming: 'status-pending',
    stream_interrupted: 'status-failed',
    success: 'status-success',
    failed: 'status-failed',
    cancelled: 'status-cancelled',
    skipped: 'status-skipped'
  }
  return classes[status] || 'status-available'
}

// 展示状态：进行中态优先（包括 started 但未 finished 的中间态），再按 HTTP 状态码兜底
const getDisplayStatus = (attempt: CandidateRecord | null | undefined): string => {
  if (!attempt) return 'available'
  const hasFinished = Boolean(attempt.finished_at)
  const isExplicitPending = (attempt.status === 'pending' || attempt.status === 'streaming') && !hasFinished
  const isImplicitPending = Boolean(
    attempt.started_at &&
      !hasFinished &&
      !['failed', 'cancelled', 'skipped', 'stream_interrupted'].includes(attempt.status),
  )

  if (isExplicitPending || isImplicitPending) {
    return 'pending'
  }
  const code = attempt.status_code
  if (typeof code === 'number') {
    if (code >= 200 && code < 300) return 'success'
    if (code >= 400) return 'failed'
  }
  return attempt.status
}
</script>

<style scoped>
.minimal-request-timeline {
  width: 100%;
}

/* 极简轨道 - 包装器实现溢出时居左、不溢出时居中 */
.minimal-track {
  display: flex;
  align-items: center;
  justify-content: safe center;
  gap: 64px;
  padding: 2rem;
  overflow-x: auto;
  overflow-y: hidden;

  /* 优化滚动体验 */
  scrollbar-width: thin; /* Firefox */
  scrollbar-color: hsl(var(--border)) transparent;
}

/* Webkit 滚动条样式 */
.minimal-track::-webkit-scrollbar {
  height: 6px;
}

.minimal-track::-webkit-scrollbar-track {
  background: transparent;
}

.minimal-track::-webkit-scrollbar-thumb {
  background: hsl(var(--border));
  border-radius: 3px;
}

.minimal-track::-webkit-scrollbar-thumb:hover {
  background: hsl(var(--muted-foreground) / 0.5);
}

.minimal-node-group {
  display: flex;
  align-items: center;
  position: relative;
  cursor: pointer;
}

/* 节点容器 */
.node-container {
  display: flex;
  flex-direction: column;
  align-items: center;
  position: relative;
}

/* 节点名称 - 绝对定位在节点上方 */
.node-label {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 50%;
  transform: translateX(-50%);
  font-size: 0.65rem;
  color: hsl(var(--muted-foreground));
  white-space: nowrap;
  max-width: 80px;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 主节点 - 同心圆（外圈轮廓 + 间隙 + 内部实心圆） */
.node-dot {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  transition: all 0.2s ease;
  z-index: 2;
  position: relative;
  overflow: visible;
  cursor: pointer;
  /* 外圈轮廓 */
  border: 2px solid currentColor;
  background: transparent;
}

/* 内部实心圆 - 使用 ::before 伪元素 */
.node-dot::before {
  content: '';
  position: absolute;
  top: 50%;
  left: 50%;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: currentColor;
  transform: translate(-50%, -50%);
}

/* 选中首次时的样式 */
.node-dot.is-first-selected {
  transform: scale(1.1);
}

/* 子节点容器 - 绝对定位在主节点下方 */
.sub-dots {
  position: absolute;
  top: calc(100% + 8px);
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  gap: 6px;
  padding: 0;
  background: transparent;
}

/* 子节点 - 增大点击区域 */
.sub-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  border: none;
  cursor: pointer;
  transition: all 0.15s ease;
  opacity: 0.5;
  position: relative;
}

/* 扩大点击热区 */
.sub-dot::before {
  content: '';
  position: absolute;
  top: -4px;
  left: -4px;
  right: -4px;
  bottom: -4px;
}

.sub-dot:hover {
  transform: scale(1.2);
  opacity: 0.9;
}

.sub-dot.active {
  opacity: 1;
  transform: scale(1.15);
  box-shadow: 0 0 0 2px hsl(var(--background)), 0 0 0 3px currentColor;
}

/* 子节点状态颜色 */
.sub-dot.status-success { background: #22c55e; color: #22c55e; }
.sub-dot.status-failed { background: #ef4444; color: #ef4444; }
.sub-dot.status-cancelled { background: #f59e0b; color: #f59e0b; }
.sub-dot.status-pending { background: #3b82f6; color: #3b82f6; }
.sub-dot.status-skipped { background: hsl(var(--primary)); color: hsl(var(--primary)); }
.sub-dot.status-available { background: #d1d5db; color: #d1d5db; }

/* 选中状态：呼吸动画 + 涟漪效果 */
.minimal-node-group.selected .node-dot {
  animation: breathe 2s ease-in-out infinite;
}

.minimal-node-group.selected .node-dot::after {
  content: '';
  position: absolute;
  top: 50%;
  left: 50%;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  border: 2px solid currentColor;
  background: transparent;
  transform: translate(-50%, -50%);
  animation: ripple 1.5s ease-out infinite;
  z-index: -1;
}

/* 悬停状态：只有放大效果 */
.minimal-node-group.hovered .node-dot {
  transform: scale(1.3);
}

@keyframes breathe {
  0%, 100% { transform: scale(1.3); }
  50% { transform: scale(1.5); }
}

@keyframes ripple {
  0% {
    transform: translate(-50%, -50%) scale(1);
    opacity: 0.4;
  }
  100% {
    transform: translate(-50%, -50%) scale(2.5);
    opacity: 0;
  }
}

/* 重试徽章 */
.retry-badge {
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  color: #fff;
  font-size: 0.6rem;
  font-weight: 700;
  z-index: 101;
  line-height: 1;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
}

/* 状态颜色 - 同心圆使用 color */
.node-dot.status-success { color: #22c55e; }
.node-dot.status-failed { color: #ef4444; }
.node-dot.status-cancelled { color: #f59e0b; }
.node-dot.status-pending { color: #3b82f6; }
.node-dot.status-skipped { color: hsl(var(--primary)); }
.node-dot.status-available { color: #d1d5db; }

/* 连接线容器 */
.node-line-wrapper {
  position: absolute;
  right: -64px;
  top: 50%;
  transform: translateY(-50%);
  width: 64px;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

.node-line {
  width: 100%;
  height: 2px;
  background: hsl(var(--border));
}

/* 格式转换分界线 */
.node-line.conversion-boundary {
  background: none;
  height: 0;
  border-top: 2px dashed hsl(var(--muted-foreground) / 0.4);
}

/* 详情面板 */
.detail-panel {
  margin-top: 1rem;
  background: hsl(var(--muted) / 0.3);
  border: 1px solid hsl(var(--border));
  border-radius: 14px;
  overflow: hidden;
}

.panel-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.5rem 0rem;
  border-bottom: 1px solid hsl(var(--border));
  background: hsl(var(--muted) / 0.4);
}

.panel-title {
  display: flex;
  align-items: center;
  gap: 0.625rem;
}

.title-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
}

.title-dot.status-success { background: #22c55e; }
.title-dot.status-failed { background: #ef4444; }
.title-dot.status-cancelled { background: #f59e0b; }
.title-dot.status-pending { background: #3b82f6; }
.title-dot.status-skipped { background: hsl(var(--primary)); }
.title-dot.status-available { background: #d1d5db; }

.title-text {
  font-weight: 600;
  font-size: 0.95rem;
}

.panel-nav {
  display: flex;
  align-items: center;
  gap: 0.375rem;
}

.nav-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: 1px solid hsl(var(--border));
  background: hsl(var(--background));
  border-radius: 6px;
  color: hsl(var(--muted-foreground));
  cursor: pointer;
  transition: all 0.15s ease;
}

.nav-btn:hover:not(:disabled) {
  background: hsl(var(--muted));
  color: hsl(var(--foreground));
  border-color: hsl(var(--muted-foreground) / 0.3);
}

.nav-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.nav-info {
  font-size: 0.8rem;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
  padding: 0 0.5rem;
  min-width: 50px;
  text-align: center;
}

.panel-body {
  padding: 0.75rem 0rem;
}

/* 头部分隔符 */
.header-divider {
  color: hsl(var(--border));
  margin: 0 0.5rem;
  font-size: 1rem;
}

/* 状态标签 */
.status-tag {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 52px;
  padding: 0.2rem 0.5rem;
  font-size: 0.75rem;
  font-weight: 600;
  border-radius: 6px;
  margin-left: 0.5rem;
}

.status-tag.status-success {
  background: #22c55e20;
  color: #22c55e;
}

.status-tag.status-failed {
  background: #ef444420;
  color: #ef4444;
}

.status-tag.status-cancelled {
  background: #f59e0b20;
  color: #f59e0b;
}

.status-tag.status-pending {
  background: #3b82f620;
  color: #3b82f6;
}

.status-tag.status-skipped {
  background: hsl(var(--primary) / 0.15);
  color: hsl(var(--primary));
}

.status-tag.status-available {
  background: hsl(var(--muted));
  color: hsl(var(--muted-foreground));
}

/* 缓存亲和标签 */
.cache-badge {
  display: inline-flex;
  align-items: center;
  padding: 0.15rem 0.5rem;
  font-size: 0.7rem;
  font-weight: 500;
  color: hsl(var(--primary));
  background: hsl(var(--primary) / 0.1);
  border: 1px solid hsl(var(--primary) / 0.2);
  border-radius: 9999px;
  margin-left: 0.75rem;
}

/* 缓存亲和提示 */
.cache-hint {
  display: inline-flex;
  align-items: center;
  padding: 0.15rem 0.5rem;
  font-size: 0.7rem;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
  background: hsl(var(--muted) / 0.5);
  border-radius: 4px;
  margin-left: 0.5rem;
}

.info-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 0.625rem 1.25rem;
}

.info-item {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.info-item.full-width {
  grid-column: 1 / -1;
}

.info-label {
  font-size: 0.7rem;
  color: hsl(var(--muted-foreground));
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-weight: 500;
}

.info-value {
  font-size: 0.9rem;
  font-weight: 500;
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.info-value.highlight {
  font-size: 1.1rem;
  font-weight: 600;
  font-family: ui-monospace, monospace;
  color: hsl(var(--primary));
}

.info-value code {
  font-size: 0.7rem;
  padding: 0.15rem 0.375rem;
  background: hsl(var(--muted));
  border-radius: 4px;
  color: hsl(var(--muted-foreground));
  font-family: ui-monospace, monospace;
}

/* 两行堆叠布局 */
.info-value-stacked {
  flex-direction: column;
  align-items: flex-start;
  gap: 0.2rem;
}

/* 格式代码 */
.format-code {
  font-size: 0.75rem;
  padding: 0.1rem 0.3rem;
  background: hsl(var(--muted));
  border-radius: 3px;
  color: hsl(var(--muted-foreground));
  font-family: ui-monospace, monospace;
}

/* Key 信息 */
.key-name {
  font-weight: 500;
}

.key-preview {
  font-size: 0.75rem;
  padding: 0.1rem 0.3rem;
  background: hsl(var(--muted));
  border-radius: 3px;
  color: hsl(var(--muted-foreground));
  font-family: ui-monospace, monospace;
}

/* 认证类型标签 */
.auth-type-tag {
  display: inline-flex;
  align-items: center;
  padding: 0.1rem 0.35rem;
  margin-left: 0.375rem;
  font-size: 0.65rem;
  font-weight: 500;
  color: hsl(var(--primary) / 0.8);
  background: hsl(var(--primary) / 0.08);
  border: 1px solid hsl(var(--primary) / 0.2);
  border-radius: 3px;
}

/* 代理信息 */
.proxy-name {
  font-weight: 500;
}

.proxy-detail {
  display: flex;
  align-items: center;
  gap: 0.375rem;
}

/* 号池调度 */
.pool-reason {
  display: flex;
  align-items: center;
  gap: 0.375rem;
}

.pool-reason-tag {
  display: inline-flex;
  align-items: center;
  padding: 0.15rem 0.5rem;
  font-size: 0.7rem;
  font-weight: 500;
  border-radius: 4px;
  white-space: nowrap;
  border: 1px solid hsl(var(--border));
}

.pool-reason-tag.pool-sticky {
  color: hsl(var(--chart-4));
  border-color: hsl(var(--chart-4) / 0.3);
  background: hsl(var(--chart-4) / 0.08);
}

.pool-reason-tag.pool-lru {
  color: hsl(var(--chart-2));
  border-color: hsl(var(--chart-2) / 0.3);
  background: hsl(var(--chart-2) / 0.08);
}

.pool-reason-tag.pool-random,
.pool-reason-tag.pool-tiebreak {
  color: hsl(var(--muted-foreground));
}

.pool-cost-warn {
  font-size: 0.65rem;
  color: hsl(var(--chart-5));
  font-weight: 500;
}

.pool-skip-type {
  font-weight: 500;
  color: hsl(var(--muted-foreground));
}

/* 能力标签 */
.capability-tags {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 0.375rem;
}

.capability-tag {
  display: inline-flex;
  align-items: center;
  padding: 0.15rem 0.5rem;
  font-size: 0.7rem;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
  white-space: nowrap;
  border-radius: 4px;
  background: transparent;
  border: 1px dashed hsl(var(--border));
  transition: all 0.15s ease;
}

/* 被请求使用的能力（高亮边框） */
.capability-tag.active {
  color: hsl(var(--primary));
  border-color: hsl(var(--primary) / 0.5);
  background: hsl(var(--primary) / 0.08);
}

/* Provider 官网链接 */
.provider-link {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0.25rem;
  margin-left: 0.25rem;
  color: hsl(var(--muted-foreground));
  border-radius: 4px;
  transition: all 0.15s ease;
}

.provider-link:hover {
  color: hsl(var(--primary));
  background: hsl(var(--primary) / 0.1);
}

/* 时间范围 */
.time-range {
  margin-top: 1.25rem;
  padding-top: 1rem;
  border-top: 1px dashed hsl(var(--border));
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
}

.time-label {
  font-size: 0.7rem;
  color: hsl(var(--muted-foreground));
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-weight: 500;
}

.time-value {
  font-size: 0.85rem;
  font-family: ui-monospace, monospace;
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.time-arrow {
  color: hsl(var(--muted-foreground));
}

/* 时间范围值 - 紧凑布局 */
.time-range-value {
  gap: 0.25rem !important;
}

/* 箭头容器 - 用于定位持续时间 */
.time-arrow-container {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

/* 持续时间 - 显示在箭头上方 */
.time-duration {
  position: absolute;
  top: -1.1rem;
  left: 50%;
  transform: translateX(-50%);
  font-size: 0.65rem;
  color: hsl(var(--muted-foreground));
  white-space: nowrap;
}

/* 用量区域 */
.usage-section {
  margin-top: 0.75rem;
  padding-top: 0.75rem;
  border-top: 1px dashed hsl(var(--border));
}

.usage-grid {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  padding: 0.5rem 0.75rem;
  background: hsl(var(--muted) / 0.2);
  border: 1px solid hsl(var(--border) / 0.5);
  border-radius: 8px;
}

.usage-row {
  display: flex;
  align-items: center;
}

.usage-item {
  display: flex;
  align-items: center;
  flex: 1;
}

.usage-label {
  font-size: 0.75rem;
  color: hsl(var(--muted-foreground));
  width: 56px;
  flex-shrink: 0;
}

.usage-tokens {
  font-size: 0.875rem;
  font-weight: 600;
  font-family: ui-monospace, monospace;
  width: 60px;
  flex-shrink: 0;
}

.usage-cost {
  font-size: 0.75rem;
  color: #16a34a;
  font-family: ui-monospace, monospace;
}

.dark .usage-cost {
  color: #4ade80;
}

.usage-divider {
  width: 1px;
  height: 16px;
  background: hsl(var(--border));
  margin: 0 1rem;
}

/* 跳过原因 */
.skip-reason {
  margin-top: 1rem;
  background: hsl(var(--muted) / 0.5);
  border-radius: 8px;
  display: flex;
  gap: 0.75rem;
  font-size: 0.85rem;
}

.reason-label {
  color: hsl(var(--muted-foreground));
  flex-shrink: 0;
}

.reason-value {
  color: hsl(var(--foreground));
}

/* 错误信息 */
.error-block {
  margin-top: 1rem;
  padding: 0.875rem;
  background: #ef444410;
  border: 1px solid #ef444430;
  border-radius: 8px;
}

.error-type {
  font-size: 0.75rem;
  font-weight: 600;
  color: #ef4444;
  margin-bottom: 0.25rem;
  text-transform: uppercase;
  letter-spacing: 0.025em;
}

.error-msg {
  font-size: 0.85rem;
  color: #dc2626;
  word-break: break-word;
}

/* 额外信息 */
.extra-block {
  margin-top: 1rem;
}

.extra-toggle {
  font-size: 0.8rem;
  color: hsl(var(--muted-foreground));
  cursor: pointer;
  padding: 0.5rem 0;
  user-select: none;
}

.extra-toggle:hover {
  color: hsl(var(--foreground));
}

.extra-json {
  margin-top: 0.5rem;
  padding: 0.75rem;
  background: hsl(var(--muted) / 0.5);
  border-radius: 8px;
  font-size: 0.75rem;
  font-family: ui-monospace, monospace;
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-all;
}

/* 动画 */
.slide-up-enter-active,
.slide-up-leave-active {
  transition: all 0.25s ease;
}

.slide-up-enter-from,
.slide-up-leave-to {
  opacity: 0;
  transform: translateY(10px);
}
</style>
