<template>
  <div class="space-y-4">
    <!-- 统计卡片 -->
    <div class="grid grid-cols-2 lg:grid-cols-4 gap-4">
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-primary/10 flex items-center justify-center">
            <Video class="w-5 h-5 text-primary" />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ stats?.total ?? '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              总任务数
            </p>
          </div>
        </div>
      </Card>
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-blue-500/10 flex items-center justify-center">
            <Loader2
              class="w-5 h-5 text-blue-500"
              :class="{ 'animate-spin': processingCount > 0 }"
            />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ processingCount || '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              生成中
            </p>
          </div>
        </div>
      </Card>
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-green-500/10 flex items-center justify-center">
            <CheckCircle class="w-5 h-5 text-green-500" />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ stats?.by_status?.completed ?? '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              已完成
            </p>
          </div>
        </div>
      </Card>
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-amber-500/10 flex items-center justify-center">
            <Calendar class="w-5 h-5 text-amber-500" />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ stats?.today_count ?? '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              今日新增
            </p>
          </div>
        </div>
      </Card>
    </div>

    <!-- 筛选栏 -->
    <Card
      variant="default"
      class="p-4"
    >
      <div class="flex flex-wrap items-center gap-2">
        <Select v-model="filterStatus">
          <SelectTrigger class="w-32 h-8 text-xs border-border/60">
            <SelectValue placeholder="状态" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">
              全部状态
            </SelectItem>
            <SelectItem value="queued">
              排队中
            </SelectItem>
            <SelectItem value="processing">
              生成中
            </SelectItem>
            <SelectItem value="completed">
              已完成
            </SelectItem>
            <SelectItem value="failed">
              失败
            </SelectItem>
            <SelectItem value="cancelled">
              已取消
            </SelectItem>
          </SelectContent>
        </Select>
        <Input
          v-model="filterModel"
          placeholder="按模型筛选"
          class="w-44 h-8 text-xs"
          @keyup.enter="applyFilters"
        />
        <Button
          variant="outline"
          size="sm"
          class="h-8"
          @click="applyFilters"
        >
          筛选
        </Button>
        <Button
          variant="ghost"
          size="sm"
          class="h-8"
          :disabled="loading"
          @click="refresh"
        >
          <RefreshCw
            class="w-3.5 h-3.5 mr-1"
            :class="{ 'animate-spin': loading }"
          />
          刷新
        </Button>
      </div>
    </Card>

    <!-- 任务表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-[76px]">
              预览
            </TableHead>
            <TableHead class="w-[24%]">
              模型 / 提示词
            </TableHead>
            <TableHead
              v-if="isAdmin"
              class="w-[12%]"
            >
              用户 / Provider
            </TableHead>
            <TableHead class="w-[13%]">
              状态
            </TableHead>
            <TableHead class="w-[11%] text-center">
              Tokens
            </TableHead>
            <TableHead class="w-[8%] text-right">
              费用
            </TableHead>
            <TableHead class="w-[12%]">
              参数 / 时间
            </TableHead>
            <TableHead class="w-[9%] text-center">
              操作
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="task in tasks"
            :key="task.id"
            class="cursor-pointer hover:bg-muted/50"
            @click="openUsageDetail(task)"
          >
            <!-- 预览 -->
            <TableCell>
              <button
                v-if="task.video_url && isCompleted(task.status)"
                type="button"
                class="relative w-16 h-10 rounded overflow-hidden bg-muted group"
                title="点击播放"
                @click.stop="openPreview(task)"
              >
                <video
                  :src="resolveVideoUrl(task)"
                  class="w-full h-full object-cover"
                  preload="metadata"
                  muted
                  playsinline
                />
                <span
                  class="absolute inset-0 flex items-center justify-center bg-black/30 group-hover:bg-black/45 transition-colors"
                >
                  <Play class="w-4 h-4 text-white" />
                </span>
              </button>
              <div
                v-else-if="isProcessing(task.status)"
                class="w-16 h-10 rounded bg-muted flex flex-col items-center justify-center gap-0.5"
              >
                <Loader2 class="w-3.5 h-3.5 text-blue-500 animate-spin" />
                <span class="text-[10px] text-muted-foreground tabular-nums">
                  {{ task.progress_percent }}%
                </span>
              </div>
              <div
                v-else-if="isFailed(task.status)"
                class="w-16 h-10 rounded bg-destructive/10 flex items-center justify-center"
              >
                <AlertCircle class="w-4 h-4 text-destructive" />
              </div>
              <div
                v-else
                class="w-16 h-10 rounded bg-muted flex items-center justify-center"
              >
                <Video class="w-4 h-4 text-muted-foreground/50" />
              </div>
            </TableCell>

            <!-- 模型 / 提示词 -->
            <TableCell>
              <div class="space-y-1 min-w-0">
                <span class="font-medium text-sm truncate block">{{ task.model || '-' }}</span>
                <p
                  class="text-xs text-muted-foreground truncate"
                  :title="task.prompt || ''"
                >
                  {{ task.prompt || '-' }}
                </p>
              </div>
            </TableCell>

            <!-- 用户 / Provider -->
            <TableCell v-if="isAdmin">
              <div class="space-y-0.5 text-xs min-w-0">
                <div class="flex items-center gap-1 truncate">
                  <User class="w-3 h-3 text-muted-foreground shrink-0" />
                  <span class="truncate">{{ task.username }}</span>
                </div>
                <div class="flex items-center gap-1 text-muted-foreground truncate">
                  <Server class="w-3 h-3 shrink-0" />
                  <span class="truncate">{{ task.provider_name }}</span>
                </div>
              </div>
            </TableCell>

            <!-- 状态 -->
            <TableCell>
              <div class="space-y-1">
                <Badge :variant="statusVariant(task.status)">
                  {{ statusLabel(task.status) }}
                </Badge>
                <div
                  v-if="isProcessing(task.status)"
                  class="w-full h-1 rounded-full bg-muted overflow-hidden"
                >
                  <div
                    class="h-full bg-blue-500 transition-all"
                    :style="{ width: `${task.progress_percent}%` }"
                  />
                </div>
                <p
                  v-if="isFailed(task.status) && task.error_message"
                  class="text-[11px] text-destructive truncate"
                  :title="task.error_message"
                >
                  {{ task.error_message }}
                </p>
              </div>
            </TableCell>

            <!-- Tokens：与使用记录页保持一致的 输入/输出 排版 -->
            <TableCell class="py-4">
              <div
                v-if="hasUsage(task)"
                class="grid w-full min-w-0 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] gap-x-1 text-xs leading-tight tabular-nums"
              >
                <span class="justify-self-end whitespace-nowrap text-right">
                  {{ formatTokens(task.input_tokens || 0) }}
                </span>
                <span class="justify-self-center text-muted-foreground">
                  /
                </span>
                <span class="justify-self-start whitespace-nowrap text-left">
                  {{ formatTokens(task.output_tokens || 0) }}
                </span>
              </div>
              <span
                v-else
                class="block text-center text-xs text-muted-foreground"
              >-</span>
            </TableCell>

            <!-- 费用 -->
            <TableCell class="text-right py-4">
              <div
                v-if="hasUsage(task)"
                class="flex flex-col items-end text-xs gap-0.5"
              >
                <span class="text-primary font-medium">{{ formatCurrency(task.cost || 0) }}</span>
                <span
                  v-if="showsActualCost(task)"
                  class="text-muted-foreground"
                >
                  {{ formatCurrency(task.actual_cost || 0) }}
                </span>
              </div>
              <span
                v-else
                class="text-xs text-muted-foreground"
              >-</span>
            </TableCell>

            <!-- 参数 / 时间 -->
            <TableCell>
              <div class="space-y-0.5 text-xs text-muted-foreground">
                <div
                  v-if="taskParams(task)"
                  class="truncate"
                  :title="taskParams(task)"
                >
                  {{ taskParams(task) }}
                </div>
                <div class="flex items-center gap-1">
                  <Clock class="w-3 h-3 shrink-0" />
                  <span>{{ formatDate(task.created_at) }}</span>
                </div>
              </div>
            </TableCell>

            <!-- 操作 -->
            <TableCell>
              <div class="flex items-center justify-center gap-1">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :disabled="!task.usage_id"
                  :title="task.usage_id ? '查看计费明细' : '任务未结算，暂无计费明细'"
                  @click.stop="openUsageDetail(task)"
                >
                  <Eye class="w-3.5 h-3.5" />
                </Button>
                <Button
                  v-if="canCancel(task.status)"
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-destructive"
                  title="取消任务"
                  @click.stop="cancelTask(task)"
                >
                  <X class="w-3.5 h-3.5" />
                </Button>
              </div>
            </TableCell>
          </TableRow>

          <TableRow v-if="!loading && tasks.length === 0">
            <TableCell
              :colspan="isAdmin ? 8 : 7"
              class="text-center py-12 text-sm text-muted-foreground"
            >
              暂无视频任务
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </Card>

    <Pagination
      v-if="total > 0"
      :current="currentPage"
      :total="total"
      :page-size="pageSize"
      cache-key="video-tasks-page-size"
      @update:current="onPageChange"
      @update:page-size="onPageSizeChange"
    />

    <!-- 视频预览 -->
    <div
      v-if="previewTask"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-6"
      @click="previewTask = null"
    >
      <div
        class="max-w-3xl w-full space-y-2"
        @click.stop
      >
        <div class="flex items-center justify-between text-white">
          <span class="text-sm truncate">{{ previewTask.model }} — {{ previewTask.prompt }}</span>
          <Button
            variant="ghost"
            size="icon"
            class="text-white hover:text-white"
            @click="previewTask = null"
          >
            <X class="w-4 h-4" />
          </Button>
        </div>
        <video
          :src="resolveVideoUrl(previewTask)"
          class="w-full rounded-lg bg-black"
          controls
          autoplay
        />
      </div>
    </div>

    <!-- 计费与请求明细，复用使用记录页的抽屉 -->
    <RequestDetailDrawer
      :is-open="usageDetailOpen"
      :request-id="usageRequestId"
      @close="usageDetailOpen = false"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import {
  videoTasksApi,
  type VideoTaskItem,
  type VideoTaskStatus,
  type VideoTaskStatsResponse,
} from '@/api/video-tasks'
import { useToast } from '@/composables/useToast'
import { useAuthStore } from '@/stores/auth'
import { formatTokens, formatCurrency } from '@/utils/format'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Badge from '@/components/ui/badge.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import Table from '@/components/ui/table.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableRow from '@/components/ui/table-row.vue'
import TableHead from '@/components/ui/table-head.vue'
import TableCell from '@/components/ui/table-cell.vue'
import Pagination from '@/components/ui/pagination.vue'
import { RequestDetailDrawer } from '@/features/usage/components'
import {
  Video,
  Play,
  Loader2,
  CheckCircle,
  Calendar,
  RefreshCw,
  User,
  Server,
  Clock,
  X,
  AlertCircle,
  Eye,
} from 'lucide-vue-next'

const props = defineProps<{ active: boolean }>()
const emit = defineEmits<{ (event: 'stats', payload: VideoTaskStatsResponse | null): void }>()

const { toast } = useToast()
const authStore = useAuthStore()
const isAdmin = computed(() => authStore.canAccessAdmin)

const loading = ref(false)
const tasks = ref<VideoTaskItem[]>([])
const stats = ref<VideoTaskStatsResponse | null>(null)
const total = ref(0)
const currentPage = ref(1)
const pageSize = ref(20)
const filterStatus = ref('all')
const filterModel = ref('')
const previewTask = ref<VideoTaskItem | null>(null)
const usageDetailOpen = ref(false)
const usageRequestId = ref<string | null>(null)

const processingCount = computed(() => {
  return stats.value?.processing_count
    ?? stats.value?.by_status?.processing
    ?? 0
})

function isCompleted(status: string): boolean {
  return status === 'completed'
}

function isProcessing(status: string): boolean {
  return ['pending', 'submitted', 'queued', 'processing'].includes(status)
}

function isFailed(status: string): boolean {
  return ['failed', 'expired'].includes(status)
}

function canCancel(status: string): boolean {
  return isProcessing(status)
}

/** Usage figures are absent until the task settles, so zero must not be shown as a real value. */
function hasUsage(task: VideoTaskItem): boolean {
  return task.total_tokens !== null && task.total_tokens !== undefined
}

/** Mirrors the usage records table: the discounted price is only worth showing when it differs. */
function showsActualCost(task: VideoTaskItem): boolean {
  return task.actual_cost !== null
    && task.actual_cost !== undefined
    && task.actual_cost !== task.cost
}

function statusVariant(status: string): 'default' | 'secondary' | 'destructive' | 'outline' {
  if (isCompleted(status)) return 'default'
  if (isFailed(status)) return 'destructive'
  if (status === 'cancelled' || status === 'deleted') return 'outline'
  return 'secondary'
}

function statusLabel(status: string): string {
  const labels: Record<string, string> = {
    pending: '待处理',
    submitted: '已提交',
    queued: '排队中',
    processing: '生成中',
    completed: '已完成',
    failed: '失败',
    cancelled: '已取消',
    expired: '已过期',
    deleted: '已删除',
  }
  return labels[status] || status
}

function taskParams(task: VideoTaskItem): string {
  const parts: string[] = []
  if (task.resolution) parts.push(task.resolution)
  if (task.aspect_ratio) parts.push(task.aspect_ratio)
  if (task.duration_seconds) parts.push(`${task.duration_seconds}s`)
  return parts.join(' · ')
}

function formatDate(dateStr: string | null | undefined): string {
  if (!dateStr) return '-'
  return new Date(dateStr).toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/**
 * Provider URLs that require upstream credentials are streamed through the
 * gateway; already-public signed URLs are used directly.
 */
function resolveVideoUrl(task: VideoTaskItem): string {
  const originalUrl = task.video_url || ''
  if (originalUrl.includes('generativelanguage.googleapis.com')) {
    return videoTasksApi.videoUrl(task.id, localStorage.getItem('access_token'))
  }
  return originalUrl
}

function openPreview(task: VideoTaskItem) {
  previewTask.value = task
}

function openUsageDetail(task: VideoTaskItem) {
  // The drawer resolves records by usage primary key, which only exists once the
  // task has settled and produced a usage row.
  const usageId = task.usage_id?.trim()
  if (!usageId) {
    toast({
      title: '暂无计费记录',
      description: '该任务还未结算，稍后再查看',
    })
    return
  }
  usageRequestId.value = usageId
  usageDetailOpen.value = true
}

async function fetchTasks() {
  loading.value = true
  try {
    const response = await videoTasksApi.list({
      status: filterStatus.value !== 'all' ? filterStatus.value as VideoTaskStatus : undefined,
      model: filterModel.value || undefined,
      page: currentPage.value,
      page_size: pageSize.value,
    })
    tasks.value = response.items
    total.value = response.total
  } catch (error: unknown) {
    toast({
      title: '获取视频任务失败',
      description: error instanceof Error ? error.message : String(error),
      variant: 'destructive',
    })
  } finally {
    loading.value = false
  }
}

async function fetchStats() {
  try {
    stats.value = await videoTasksApi.getStats()
    emit('stats', stats.value)
  } catch {
    stats.value = null
    emit('stats', null)
  }
}

async function refresh() {
  await Promise.all([fetchTasks(), fetchStats()])
}

function applyFilters() {
  currentPage.value = 1
  fetchTasks()
}

function onPageChange(page: number) {
  currentPage.value = page
  fetchTasks()
}

function onPageSizeChange(size: number) {
  pageSize.value = size
  currentPage.value = 1
  fetchTasks()
}

async function cancelTask(task: VideoTaskItem) {
  try {
    await videoTasksApi.cancel(task.id)
    toast({ title: '已请求取消任务' })
    await refresh()
  } catch (error: unknown) {
    toast({
      title: '取消任务失败',
      description: error instanceof Error ? error.message : String(error),
      variant: 'destructive',
    })
  }
}

watch(() => filterStatus.value, () => {
  currentPage.value = 1
  fetchTasks()
})

// Only load once the tab becomes visible, so the inactive panel costs nothing.
watch(() => props.active, (active) => {
  if (active && tasks.value.length === 0) {
    refresh()
  }
})

onMounted(() => {
  if (props.active) {
    refresh()
  }
})

defineExpose({ refresh })
</script>
