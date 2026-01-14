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
                    getStatusColorClass(group.primaryStatus),
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
                      getStatusColorClass(attempt.status),
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
                class="node-line"
              />
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
                    :class="getStatusColorClass(currentAttempt.status)"
                  />
                  <span class="title-text">{{ selectedGroup.providerName }}</span>
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
                    :class="getStatusColorClass(currentAttempt.status)"
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
                    v-if="currentAttempt.key_name || currentAttempt.key_id"
                    class="info-item"
                  >
                    <span class="info-label">密钥</span>
                    <span class="info-value">
                      <span class="key-name">{{ currentAttempt.key_name || '未知' }}</span>
                      <template v-if="currentAttempt.key_preview">
                        <Separator
                          orientation="vertical"
                          class="h-3 mx-2"
                        />
                        <code class="key-preview">{{ currentAttempt.key_preview }}</code>
                      </template>
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
import Separator from '@/components/ui/separator.vue'
import { ChevronLeft, ChevronRight, ExternalLink } from 'lucide-vue-next'
import { requestTraceApi, type RequestTrace, type CandidateRecord } from '@/api/requestTrace'
import { log } from '@/utils/logger'

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
  /** 用量和费用数据 */
  usageData?: UsageData | null
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
const getFinalStatusBadgeVariant = (status: string): any => {
  const variants: Record<string, string> = {
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

// 候选时间线（按实际执行顺序排序）
const timeline = computed<CandidateRecord[]>(() => {
  if (!trace.value) return []
  return [...trace.value.candidates]
    .filter(c => ['success', 'failed', 'skipped', 'available', 'pending', 'streaming'].includes(c.status))
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

// 将相同 Provider 的所有请求合并为组（同提供商的 Key 放在子节点）
const groupedTimeline = computed<NodeGroup[]>(() => {
  if (!timeline.value || timeline.value.length === 0) return []

  const groups: NodeGroup[] = []
  let currentGroup: NodeGroup | null = null

  timeline.value.forEach((candidate, index) => {
    // 使用 provider_name 作为分组 key（同一个提供商的所有 Key 合并）
    const providerKey = candidate.provider_name || '未知'

    // 如果属于同一个 Provider，合并到当前组
    if (currentGroup && currentGroup.id === providerKey) {
      currentGroup.allAttempts.push(candidate)
      currentGroup.retryCount++
      currentGroup.endIndex = index
      currentGroup.totalLatency += candidate.latency_ms || 0
      // 如果任一尝试成功，更新主状态
      if (candidate.status === 'success') {
        currentGroup.primaryStatus = 'success'
      }
    } else {
      // 新建一个组
      currentGroup = {
        id: providerKey,
        providerName: candidate.provider_name || '未知',
        primary: candidate,
        primaryStatus: candidate.status,
        allAttempts: [candidate],
        retryCount: 0,
        totalLatency: candidate.latency_ms || 0,
        startIndex: index,
        endIndex: index
      }
      groups.push(currentGroup)
    }
  })

  return groups
})

// 计算链路总耗时（使用成功候选的 latency_ms 字段）
// 优先使用 latency_ms，因为它与 Usage.response_time_ms 使用相同的时间基准
// 避免 finished_at - started_at 带来的额外延迟（数据库操作时间）
const totalTraceLatency = computed(() => {
  if (!timeline.value || timeline.value.length === 0) return 0

  // 查找成功的候选，使用其 latency_ms
  const successCandidate = timeline.value.find(c => c.status === 'success')
  if (successCandidate?.latency_ms != null) {
    return successCandidate.latency_ms
  }

  // 如果没有成功的候选，查找失败但有 latency_ms 的候选
  const failedWithLatency = timeline.value.find(c => c.status === 'failed' && c.latency_ms != null)
  if (failedWithLatency?.latency_ms != null) {
    return failedWithLatency.latency_ms
  }

  // 回退：使用 finished_at - started_at 计算
  let earliestStart: number | null = null
  let latestEnd: number | null = null

  for (const candidate of timeline.value) {
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
const loadTrace = async () => {
  if (!props.requestId) return

  loading.value = true
  error.value = null

  try {
    trace.value = await requestTraceApi.getRequestTrace(props.requestId)
  } catch (err: any) {
    error.value = err.response?.data?.detail || err.message || '加载失败'
    log.error('加载请求追踪失败:', err)
  } finally {
    loading.value = false
  }
}

// 监听 groupedTimeline 变化，自动选择最有意义的组
watch(groupedTimeline, (newGroups) => {
  if (!newGroups || newGroups.length === 0) return

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
    selectedAttemptIndex.value = newGroups[activeIdx].allAttempts.length - 1
    return
  }

  // 查找最后一个有效结果的组（failed 优先于 skipped/available）
  // 从后往前找第一个 failed 的组
  for (let i = newGroups.length - 1; i >= 0; i--) {
    const group = newGroups[i]
    if (group.primaryStatus === 'failed') {
      selectedGroupIndex.value = i
      // 选中最后一个失败的尝试
      const failedIdx = group.allAttempts.findLastIndex((a: CandidateRecord) => a.status === 'failed')
      selectedAttemptIndex.value = failedIdx >= 0 ? failedIdx : group.allAttempts.length - 1
      return
    }
  }

  // 都没有则选择最后一个组
  selectedGroupIndex.value = newGroups.length - 1
  selectedAttemptIndex.value = newGroups[newGroups.length - 1].allAttempts.length - 1
}, { immediate: true })

// 监听 requestId 变化
watch(() => props.requestId, () => {
  selectedGroupIndex.value = 0
  selectedAttemptIndex.value = 0
  loadTrace()
}, { immediate: true })

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
    pending: '等待中',
    streaming: '传输中',
    success: '成功',
    failed: '失败',
    skipped: '跳过'
  }
  return labels[status] || status
}

// 获取状态颜色类
const getStatusColorClass = (status: string) => {
  const classes: Record<string, string> = {
    available: 'status-available',
    pending: 'status-pending',
    streaming: 'status-pending',
    success: 'status-success',
    failed: 'status-failed',
    skipped: 'status-skipped'
  }
  return classes[status] || 'status-available'
}
</script>

<style scoped>
.minimal-request-timeline {
  width: 100%;
}

/* 极简轨道 */
.minimal-track {
  display: flex;
  align-items: center;
  justify-content: space-between;
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
.sub-dot.status-pending { background: #3b82f6; color: #3b82f6; }
.sub-dot.status-skipped { background: #1f2937; color: #1f2937; }
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
.node-dot.status-pending { color: #3b82f6; }
.node-dot.status-skipped { color: #1f2937; }
.node-dot.status-available { color: #d1d5db; }

.node-line {
  position: absolute;
  right: -64px;
  top: 50%;
  transform: translateY(-50%);
  width: 64px;
  height: 2px;
  background: hsl(var(--border));
  z-index: 1;
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
.title-dot.status-pending { background: #3b82f6; }
.title-dot.status-skipped { background: #1f2937; }
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
  color: #16a34a;
}

.status-tag.status-failed {
  background: #ef444420;
  color: #dc2626;
}

.status-tag.status-pending {
  background: #3b82f620;
  color: #2563eb;
}

.status-tag.status-skipped {
  background: #1f293720;
  color: #1f2937;
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

/* Key 信息 */
.key-name {
  font-weight: 500;
}

.key-preview {
  font-size: 0.8rem;
  padding: 0.1rem 0.3rem;
  background: hsl(var(--muted));
  border-radius: 3px;
  color: hsl(var(--muted-foreground));
  font-family: ui-monospace, monospace;
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
