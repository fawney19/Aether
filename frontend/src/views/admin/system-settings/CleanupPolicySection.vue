<template>
  <CardSection
    title="请求记录清理策略"
    description="配置请求记录的分级保留和自动清理"
  >
    <template #actions>
      <div class="flex items-center gap-4">
        <div class="flex items-center gap-2">
          <Switch
            id="enable-auto-cleanup"
            :model-value="enableAutoCleanup"
            @update:model-value="$emit('toggleAutoCleanup', $event)"
          />
          <div>
            <Label
              for="enable-auto-cleanup"
              class="text-sm cursor-pointer"
            >
              启用自动清理
            </Label>
            <p class="text-xs text-muted-foreground">
              每天凌晨执行
            </p>
          </div>
        </div>
        <Button
          variant="destructive"
          size="sm"
          :disabled="manualCleanupRunning"
          @click="openManualCleanupDialog"
        >
          <Trash2 class="w-3.5 h-3.5 mr-1.5" />
          {{ manualCleanupRunning ? '清理中…' : '立即清理' }}
        </Button>
        <Button
          size="sm"
          :disabled="loading || !hasChanges"
          @click="$emit('save')"
        >
          {{ loading ? '保存中...' : '保存' }}
        </Button>
      </div>
    </template>
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <div>
        <Label
          for="detail-log-retention-days"
          class="block text-sm font-medium"
        >
          详细记录保留天数
        </Label>
        <Input
          id="detail-log-retention-days"
          :model-value="detailLogRetentionDays"
          type="number"
          placeholder="7"
          class="mt-1"
          @update:model-value="$emit('update:detailLogRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          超过后压缩 body 字段
        </p>
      </div>

      <div>
        <Label
          for="compressed-log-retention-days"
          class="block text-sm font-medium"
        >
          压缩记录保留天数
        </Label>
        <Input
          id="compressed-log-retention-days"
          :model-value="compressedLogRetentionDays"
          type="number"
          placeholder="30"
          class="mt-1"
          @update:model-value="$emit('update:compressedLogRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          超过后删除 body 字段
        </p>
      </div>

      <div>
        <Label
          for="header-retention-days"
          class="block text-sm font-medium"
        >
          请求头保留天数
        </Label>
        <Input
          id="header-retention-days"
          :model-value="headerRetentionDays"
          type="number"
          placeholder="90"
          class="mt-1"
          @update:model-value="$emit('update:headerRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          超过后清空 headers 字段
        </p>
      </div>

      <div>
        <Label
          for="log-retention-days"
          class="block text-sm font-medium"
        >
          请求记录保存天数
        </Label>
        <Input
          id="log-retention-days"
          :model-value="logRetentionDays"
          type="number"
          placeholder="365"
          class="mt-1"
          @update:model-value="$emit('update:logRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          超过后删除整条记录
        </p>
      </div>

      <div>
        <Label
          for="cleanup-batch-size"
          class="block text-sm font-medium"
        >
          每批次清理记录数
        </Label>
        <Input
          id="cleanup-batch-size"
          :model-value="cleanupBatchSize"
          type="number"
          placeholder="1000"
          class="mt-1"
          @update:model-value="$emit('update:cleanupBatchSize', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          避免单次操作过大影响性能
        </p>
      </div>

      <div>
        <Label
          for="audit-log-retention-days"
          class="block text-sm font-medium"
        >
          审计日志保留天数
        </Label>
        <Input
          id="audit-log-retention-days"
          :model-value="auditLogRetentionDays"
          type="number"
          placeholder="30"
          class="mt-1"
          @update:model-value="$emit('update:auditLogRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          超过后删除审计日志记录
        </p>
      </div>

      <div>
        <Label
          for="request-candidates-retention-days"
          class="block text-sm font-medium"
        >
          候选记录保留天数
        </Label>
        <Input
          id="request-candidates-retention-days"
          :model-value="requestCandidatesRetentionDays"
          type="number"
          placeholder="30"
          class="mt-1"
          @update:model-value="$emit('update:requestCandidatesRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          过期后按时间窗口批量清理 request_candidates 审计记录
        </p>
      </div>

      <div>
        <Label
          for="request-candidates-cleanup-batch-size"
          class="block text-sm font-medium"
        >
          候选记录清理批次
        </Label>
        <Input
          id="request-candidates-cleanup-batch-size"
          :model-value="requestCandidatesCleanupBatchSize"
          type="number"
          placeholder="5000"
          class="mt-1"
          @update:model-value="$emit('update:requestCandidatesCleanupBatchSize', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          独立控制候选记录大表清理节奏，不再跟随 Key 删除联动
        </p>
      </div>

      <div>
        <Label
          for="proxy-node-metrics-1m-retention-days"
          class="block text-sm font-medium"
        >
          代理 1m 指标保留天数
        </Label>
        <Input
          id="proxy-node-metrics-1m-retention-days"
          :model-value="proxyNodeMetrics1mRetentionDays"
          type="number"
          min="1"
          max="365"
          placeholder="30"
          class="mt-1"
          @update:model-value="$emit('update:proxyNodeMetrics1mRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          用于代理节点稳定性分钟级图表，后端最少保留 1 天
        </p>
      </div>

      <div>
        <Label
          for="proxy-node-metrics-1h-retention-days"
          class="block text-sm font-medium"
        >
          代理 1h 指标保留天数
        </Label>
        <Input
          id="proxy-node-metrics-1h-retention-days"
          :model-value="proxyNodeMetrics1hRetentionDays"
          type="number"
          min="1"
          max="1095"
          placeholder="180"
          class="mt-1"
          @update:model-value="$emit('update:proxyNodeMetrics1hRetentionDays', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          小时级聚合用于长期趋势，不能短于 1m 指标保留天数
        </p>
      </div>

      <div>
        <Label
          for="proxy-node-metrics-cleanup-batch-size"
          class="block text-sm font-medium"
        >
          代理指标清理批次
        </Label>
        <Input
          id="proxy-node-metrics-cleanup-batch-size"
          :model-value="proxyNodeMetricsCleanupBatchSize"
          type="number"
          min="1"
          max="50000"
          placeholder="5000"
          class="mt-1"
          @update:model-value="$emit('update:proxyNodeMetricsCleanupBatchSize', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          独立限制 proxy_node_metrics 表的单批删除数量
        </p>
      </div>
    </div>

    <!-- 清理策略说明 -->
    <div class="mt-4 p-4 bg-muted/50 rounded-lg">
      <h4 class="text-sm font-medium mb-2">
        清理策略说明
      </h4>
      <div class="text-xs text-muted-foreground space-y-1">
        <p>1. <strong>详细日志阶段</strong>: 保留完整的 request_body 和 response_body</p>
        <p>2. <strong>压缩日志阶段</strong>: body 字段被压缩存储，节省空间</p>
        <p>3. <strong>统计阶段</strong>: 仅保留 tokens、成本等统计信息</p>
        <p>4. <strong>归档删除</strong>: 超过保留期限后完全删除记录</p>
        <p>5. <strong>候选记录</strong>: 独立按保留天数清理 request_candidates 审计记录，不再跟随 Key 删除联动</p>
        <p>6. <strong>审计日志</strong>: 独立清理，记录用户登录、操作等安全事件</p>
        <p>7. <strong>代理指标</strong>: 仅保留 1m/1h 聚合桶，清理任务按批次删除过期桶</p>
      </div>
    </div>

    <ManualCleanupConfirmDialog
      :open="manualCleanupDialogOpen"
      @update:open="manualCleanupDialogOpen = $event"
      @running-change="manualCleanupRunning = $event"
      @completed="handleManualCleanupCompleted"
    />

    <div
      v-if="manualCleanupResult"
      class="mt-4 rounded-md border border-border bg-muted/30 px-4 py-3 text-sm"
    >
      <div class="font-medium">
        {{ manualCleanupResult.title }}
      </div>
      <div
        v-if="manualCleanupResult.description"
        class="mt-1 text-xs text-muted-foreground"
      >
        {{ manualCleanupResult.description }}
      </div>
    </div>

    <div class="mt-4 border border-border rounded-lg overflow-hidden">
      <div class="flex flex-wrap items-start justify-between gap-3 px-4 py-3 border-b border-border">
        <div class="min-w-0 flex-1 basis-48">
          <h4 class="text-sm font-medium">
            最近清理记录
          </h4>
          <p class="text-xs text-muted-foreground">
            自动清理、手动系统清理和请求体后台任务的执行结果
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          class="shrink-0"
          :disabled="cleanupRunsLoading"
          @click="loadCleanupRuns"
        >
          <RefreshCw
            class="w-3.5 h-3.5 mr-1.5"
            :class="{ 'animate-spin': cleanupRunsLoading }"
          />
          刷新
        </Button>
      </div>
      <div
        v-if="cleanupRuns.length === 0 && !cleanupRunsLoading"
        class="px-4 py-6 text-sm text-muted-foreground"
      >
        暂无清理记录
      </div>
      <div
        v-else
        class="overflow-x-auto"
      >
        <table class="w-full text-sm">
          <thead class="bg-muted/30 text-xs text-muted-foreground">
            <tr>
              <th class="px-4 py-2 text-left font-medium">
                时间
              </th>
              <th class="px-4 py-2 text-left font-medium">
                类型
              </th>
              <th class="px-4 py-2 text-left font-medium">
                来源
              </th>
              <th class="px-4 py-2 text-left font-medium">
                状态
              </th>
              <th class="px-4 py-2 text-left font-medium">
                结果
              </th>
              <th class="px-4 py-2 text-right font-medium">
                耗时
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="run in cleanupRuns"
              :key="run.id"
              class="border-t border-border"
            >
              <td class="px-4 py-2 whitespace-nowrap">
                {{ formatRunTime(run.started_at_unix_secs) }}
              </td>
              <td class="px-4 py-2 whitespace-nowrap">
                {{ cleanupKindLabel(run.kind) }}
              </td>
              <td class="px-4 py-2 whitespace-nowrap text-muted-foreground">
                {{ run.trigger === 'manual' ? '手动' : '自动' }}
              </td>
              <td class="px-4 py-2 whitespace-nowrap">
                <span :class="cleanupStatusClass(run.status)">
                  {{ cleanupStatusLabel(run.status) }}
                </span>
              </td>
              <td class="px-4 py-2 min-w-[18rem]">
                <div>{{ run.error || run.message }}</div>
                <div class="text-xs text-muted-foreground">
                  {{ cleanupSummaryText(run.summary) }}
                </div>
              </td>
              <td class="px-4 py-2 text-right whitespace-nowrap text-muted-foreground">
                {{ formatDuration(run.duration_ms) }}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { RefreshCw, Trash2 } from 'lucide-vue-next'
import { adminApi, type CleanupRunRecord } from '@/api/admin'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Switch from '@/components/ui/switch.vue'
import { CardSection } from '@/components/layout'
import ManualCleanupConfirmDialog from './ManualCleanupConfirmDialog.vue'
import { useToast } from '@/composables/useToast'

defineProps<{
  enableAutoCleanup: boolean
  detailLogRetentionDays: number
  compressedLogRetentionDays: number
  headerRetentionDays: number
  logRetentionDays: number
  cleanupBatchSize: number
  auditLogRetentionDays: number
  requestCandidatesRetentionDays: number
  requestCandidatesCleanupBatchSize: number
  proxyNodeMetrics1mRetentionDays: number
  proxyNodeMetrics1hRetentionDays: number
  proxyNodeMetricsCleanupBatchSize: number
  loading: boolean
  hasChanges: boolean
}>()

defineEmits<{
  save: []
  toggleAutoCleanup: [enabled: boolean]
  'update:detailLogRetentionDays': [value: number]
  'update:compressedLogRetentionDays': [value: number]
  'update:headerRetentionDays': [value: number]
  'update:logRetentionDays': [value: number]
  'update:cleanupBatchSize': [value: number]
  'update:auditLogRetentionDays': [value: number]
  'update:requestCandidatesRetentionDays': [value: number]
  'update:requestCandidatesCleanupBatchSize': [value: number]
  'update:proxyNodeMetrics1mRetentionDays': [value: number]
  'update:proxyNodeMetrics1hRetentionDays': [value: number]
  'update:proxyNodeMetricsCleanupBatchSize': [value: number]
}>()

const cleanupRuns = ref<CleanupRunRecord[]>([])
const cleanupRunsLoading = ref(false)
let cleanupRunsTimer: number | null = null

const manualCleanupDialogOpen = ref(false)
const manualCleanupRunning = ref(false)
const manualCleanupResult = ref<{ title: string; description?: string } | null>(null)
const toast = useToast()

function openManualCleanupDialog() {
  if (manualCleanupRunning.value) return
  manualCleanupDialogOpen.value = true
}

function handleManualCleanupCompleted(task: CleanupRunRecord) {
  manualCleanupRunning.value = false
  manualCleanupResult.value = {
    title: task.message,
    description: cleanupSummaryText(task.summary),
  }
  if (task.status === 'failed') {
    toast.error(task.error || task.message)
  } else {
    toast.success(task.message)
  }
  void loadCleanupRuns()
}

async function loadCleanupRuns() {
  cleanupRunsLoading.value = true
  try {
    const response = await adminApi.getCleanupRuns()
    cleanupRuns.value = response.items.slice(0, 10)
  } finally {
    cleanupRunsLoading.value = false
  }
}

function cleanupKindLabel(kind: string): string {
  const labels: Record<string, string> = {
    usage_cleanup: '请求记录',
    audit_cleanup: '审计日志',
    request_candidate_cleanup: '候选记录',
    request_bodies: '请求体',
    config_purge: '配置清空',
    users_purge: '用户清空',
    usage_purge: '使用记录清空',
    audit_logs_purge: '审计日志清空',
    stats_purge: '统计聚合清空',
    system_cleanup: '系统清理',
  }
  return labels[kind] || kind
}

function cleanupStatusLabel(status: string): string {
  if (status === 'processing') return '执行中'
  if (status === 'failed') return '失败'
  return '完成'
}

function cleanupStatusClass(status: string): string {
  if (status === 'processing') return 'text-amber-500'
  if (status === 'failed') return 'text-destructive'
  return 'text-emerald-500'
}

function formatRunTime(value: number): string {
  if (!value) return '-'
  return new Date(value * 1000).toLocaleString()
}

function formatDuration(value: number | null): string {
  if (value === null || value === undefined) return '-'
  if (value < 1000) return `${value}ms`
  return `${(value / 1000).toFixed(1)}s`
}

function cleanupSummaryText(summary: Record<string, unknown>): string {
  const total = typeof summary.total === 'number' ? summary.total : null
  if (total !== null) return `影响 ${total} 行`

  const entries = Object.entries(summary)
    .filter(([, value]) => typeof value === 'number' && value > 0)
    .map(([key, value]) => `${summaryLabel(key)} ${value}`)
  return entries.length > 0 ? entries.join(' / ') : '无数据变更'
}

function summaryLabel(key: string): string {
  const labels: Record<string, string> = {
    body_externalized: '详细体',
    legacy_body_refs_migrated: '迁移',
    body_cleaned: '清体',
    header_cleaned: '清头',
    keys_cleaned: 'Key',
    records_deleted: '删记录',
    audit_logs_deleted: '删日志',
    request_candidates_deleted: '删候选',
  }
  return labels[key] || key
}

onMounted(() => {
  void loadCleanupRuns()
  cleanupRunsTimer = window.setInterval(() => {
    void loadCleanupRuns()
  }, 15_000)
})

onBeforeUnmount(() => {
  if (cleanupRunsTimer) {
    window.clearInterval(cleanupRunsTimer)
    cleanupRunsTimer = null
  }
})
</script>
