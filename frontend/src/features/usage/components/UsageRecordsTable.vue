<template>
  <TableCard title="使用记录">
    <template #actions>
      <!-- 时间段筛选 -->
      <Select
        v-model:open="periodSelectOpen"
        :model-value="selectedPeriod"
        @update:model-value="$emit('update:selectedPeriod', $event)"
      >
        <SelectTrigger class="w-24 sm:w-32 h-8 text-xs border-border/60">
          <SelectValue placeholder="选择时间段" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="today">
            今天
          </SelectItem>
          <SelectItem value="yesterday">
            昨天
          </SelectItem>
          <SelectItem value="last7days">
            最近7天
          </SelectItem>
          <SelectItem value="last30days">
            最近30天
          </SelectItem>
          <SelectItem value="last90days">
            最近90天
          </SelectItem>
        </SelectContent>
      </Select>

      <!-- 分隔线 -->
      <div class="hidden sm:block h-4 w-px bg-border" />

      <!-- 用户筛选（仅管理员可见） -->
      <Select
        v-if="isAdmin && availableUsers.length > 0"
        v-model:open="filterUserSelectOpen"
        :model-value="filterUser"
        @update:model-value="$emit('update:filterUser', $event)"
      >
        <SelectTrigger class="w-24 sm:w-36 h-8 text-xs border-border/60">
          <SelectValue placeholder="全部用户" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__all__">
            全部用户
          </SelectItem>
          <SelectItem
            v-for="user in availableUsers"
            :key="user.id"
            :value="user.id"
          >
            {{ user.username || user.email }}
          </SelectItem>
        </SelectContent>
      </Select>

      <!-- 模型筛选 -->
      <Select
        v-model:open="filterModelSelectOpen"
        :model-value="filterModel"
        @update:model-value="$emit('update:filterModel', $event)"
      >
        <SelectTrigger class="w-24 sm:w-40 h-8 text-xs border-border/60">
          <SelectValue placeholder="全部模型" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__all__">
            全部模型
          </SelectItem>
          <SelectItem
            v-for="model in availableModels"
            :key="model"
            :value="model"
          >
            {{ model.replace('claude-', '') }}
          </SelectItem>
        </SelectContent>
      </Select>

      <!-- 提供商筛选 -->
      <Select
        v-model:open="filterProviderSelectOpen"
        :model-value="filterProvider"
        @update:model-value="$emit('update:filterProvider', $event)"
      >
        <SelectTrigger class="w-24 sm:w-32 h-8 text-xs border-border/60">
          <SelectValue placeholder="全部提供商" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__all__">
            全部提供商
          </SelectItem>
          <SelectItem
            v-for="provider in availableProviders"
            :key="provider"
            :value="provider"
          >
            {{ provider }}
          </SelectItem>
        </SelectContent>
      </Select>

      <!-- 状态筛选 -->
      <Select
        v-model:open="filterStatusSelectOpen"
        :model-value="filterStatus"
        @update:model-value="$emit('update:filterStatus', $event)"
      >
        <SelectTrigger class="w-20 sm:w-28 h-8 text-xs border-border/60">
          <SelectValue placeholder="全部状态" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__all__">
            全部状态
          </SelectItem>
          <SelectItem value="active">
            进行中
          </SelectItem>
          <SelectItem value="pending">
            等待中
          </SelectItem>
          <SelectItem value="streaming">
            流式传输
          </SelectItem>
          <SelectItem value="completed">
            已完成
          </SelectItem>
          <SelectItem value="failed">
            已失败
          </SelectItem>
        </SelectContent>
      </Select>

      <!-- 分隔线 -->
      <div class="hidden sm:block h-4 w-px bg-border" />

      <!-- 刷新按钮 -->
      <RefreshButton
        :loading="loading"
        @click="$emit('refresh')"
      />
    </template>

    <Table>
      <TableHeader>
        <TableRow class="border-b border-border/60 hover:bg-transparent">
          <TableHead class="h-12 font-semibold w-[70px]">
            时间
          </TableHead>
          <TableHead
            v-if="isAdmin"
            class="h-12 font-semibold w-[100px]"
          >
            用户
          </TableHead>
          <TableHead class="h-12 font-semibold w-[140px]">
            模型
          </TableHead>
          <TableHead
            v-if="isAdmin"
            class="h-12 font-semibold w-[100px]"
          >
            提供商
          </TableHead>
          <TableHead class="h-12 font-semibold w-[80px]">
            API格式
          </TableHead>
          <TableHead class="h-12 font-semibold w-[50px] text-center">
            类型
          </TableHead>
          <TableHead class="h-12 font-semibold w-[140px] text-right">
            Tokens
          </TableHead>
          <TableHead class="h-12 font-semibold w-[100px] text-right">
            费用
          </TableHead>
          <TableHead class="h-12 font-semibold w-[70px] text-right">
            <div class="flex flex-col items-end text-xs gap-0.5">
              <span>首字</span>
              <span class="text-muted-foreground font-normal">总耗时</span>
            </div>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-if="records.length === 0">
          <TableCell
            :colspan="isAdmin ? 9 : 7"
            class="text-center py-12 text-muted-foreground"
          >
            暂无请求记录
          </TableCell>
        </TableRow>
        <TableRow
          v-for="record in records"
          v-else
          :key="record.id"
          :class="isAdmin ? 'cursor-pointer border-b border-border/40 hover:bg-muted/30 transition-colors h-[72px]' : 'border-b border-border/40 hover:bg-muted/30 transition-colors h-[72px]'"
          @mousedown="handleMouseDown"
          @click="handleRowClick($event, record.id)"
        >
          <TableCell class="text-xs py-4 w-[70px]">
            {{ formatDateTime(record.created_at) }}
          </TableCell>
          <TableCell
            v-if="isAdmin"
            class="py-4 w-[100px] truncate"
            :title="record.username || record.user_email || (record.user_id ? `User ${record.user_id}` : '已删除用户')"
          >
            {{ record.username || record.user_email || (record.user_id ? `User ${record.user_id}` : '已删除用户') }}
          </TableCell>
          <TableCell
            class="font-medium py-4 w-[140px]"
            :title="getModelTooltip(record)"
          >
            <div
              v-if="getActualModel(record)"
              class="flex flex-col text-xs gap-0.5"
            >
              <div class="flex items-center gap-1 truncate">
                <span class="truncate">{{ record.model }}</span>
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  viewBox="0 0 20 20"
                  fill="currentColor"
                  class="w-3 h-3 text-muted-foreground flex-shrink-0"
                >
                  <path
                    fill-rule="evenodd"
                    d="M3 10a.75.75 0 01.75-.75h10.638L10.23 5.29a.75.75 0 111.04-1.08l5.5 5.25a.75.75 0 010 1.08l-5.5 5.25a.75.75 0 11-1.04-1.08l4.158-3.96H3.75A.75.75 0 013 10z"
                    clip-rule="evenodd"
                  />
                </svg>
              </div>
              <span class="text-muted-foreground truncate">{{ getActualModel(record) }}</span>
            </div>
            <span
              v-else
              class="truncate block"
            >{{ record.model }}</span>
          </TableCell>
          <TableCell
            v-if="isAdmin"
            class="py-4 w-[60px]"
          >
            <div class="flex flex-col text-xs gap-0.5">
              <div class="flex items-center gap-1">
                <span>{{ record.provider }}</span>
                <span
                  v-if="record.has_fallback"
                  class="inline-flex items-center justify-center w-4 h-4 text-xs text-amber-600 dark:text-amber-400"
                  title="此请求发生了 Provider 切换"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    viewBox="0 0 20 20"
                    fill="currentColor"
                    class="w-4 h-4"
                  >
                    <path
                      fill-rule="evenodd"
                      d="M15.312 11.424a5.5 5.5 0 01-9.201 2.466l-.312-.311h2.433a.75.75 0 000-1.5H3.989a.75.75 0 00-.75.75v4.242a.75.75 0 001.5 0v-2.43l.31.31a7 7 0 0011.712-3.138.75.75 0 00-1.449-.39zm1.23-3.723a.75.75 0 00.219-.53V2.929a.75.75 0 00-1.5 0V5.36l-.31-.31A7 7 0 003.239 8.188a.75.75 0 101.448.389A5.5 5.5 0 0113.89 6.11l.311.31h-2.432a.75.75 0 000 1.5h4.243a.75.75 0 00.53-.219z"
                      clip-rule="evenodd"
                    />
                  </svg>
                </span>
              </div>
              <span
                v-if="record.api_key_name"
                class="text-muted-foreground truncate"
                :title="record.api_key_name"
              >
                {{ record.api_key_name }}
                <span
                  v-if="record.rate_multiplier && record.rate_multiplier !== 1.0"
                  class="text-foreground/60"
                >({{ record.rate_multiplier }}x)</span>
              </span>
            </div>
          </TableCell>
          <TableCell class="py-4 w-[80px]">
            <span
              v-if="record.api_format"
              class="inline-flex items-center px-2 py-0.5 rounded-full border border-border/60 text-[10px] font-medium whitespace-nowrap text-muted-foreground"
              :title="record.api_format"
            >
              {{ formatApiFormat(record.api_format) }}
            </span>
            <span
              v-else
              class="text-muted-foreground text-xs"
            >-</span>
          </TableCell>
          <TableCell class="text-center py-4 w-[50px]">
            <!-- 优先显示请求状态 -->
            <Badge
              v-if="record.status === 'pending'"
              variant="outline"
              class="whitespace-nowrap animate-pulse border-muted-foreground/30 text-muted-foreground"
            >
              等待中
            </Badge>
            <Badge
              v-else-if="record.status === 'streaming'"
              variant="outline"
              class="whitespace-nowrap animate-pulse border-primary/50 text-primary"
            >
              传输中
            </Badge>
            <Badge
              v-else-if="record.status === 'failed' || (record.status_code && record.status_code >= 400) || record.error_message"
              variant="destructive"
              class="whitespace-nowrap"
            >
              失败
            </Badge>
            <Badge
              v-else-if="record.is_stream"
              variant="secondary"
              class="whitespace-nowrap"
            >
              流式
            </Badge>
            <Badge
              v-else
              variant="outline"
              class="whitespace-nowrap border-border/60 text-muted-foreground"
            >
              标准
            </Badge>
          </TableCell>
          <TableCell class="text-right py-4 w-[140px]">
            <div class="flex flex-col items-end text-xs gap-0.5">
              <div class="flex items-center gap-1">
                <span>{{ formatTokens(record.input_tokens || 0) }}</span>
                <span class="text-muted-foreground">/</span>
                <span>{{ formatTokens(record.output_tokens || 0) }}</span>
              </div>
              <div class="flex items-center gap-1 text-muted-foreground">
                <span :class="record.cache_creation_input_tokens ? 'text-foreground/70' : ''">{{ record.cache_creation_input_tokens ? formatTokens(record.cache_creation_input_tokens) : '-' }}</span>
                <span>/</span>
                <span :class="record.cache_read_input_tokens ? 'text-foreground/70' : ''">{{ record.cache_read_input_tokens ? formatTokens(record.cache_read_input_tokens) : '-' }}</span>
              </div>
            </div>
          </TableCell>
          <TableCell class="text-right py-4 w-[100px]">
            <div class="flex flex-col items-end text-xs gap-0.5">
              <span class="text-primary font-medium">{{ formatCurrency(record.cost || 0) }}</span>
              <span
                v-if="showActualCost && record.actual_cost !== undefined"
                class="text-muted-foreground"
              >
                {{ formatCurrency(record.actual_cost) }}
              </span>
            </div>
          </TableCell>
          <TableCell class="text-right py-4 w-[70px]">
            <div
              v-if="record.status === 'pending' || record.status === 'streaming'"
              class="flex flex-col items-end text-xs gap-0.5"
            >
              <span class="text-primary tabular-nums">
                {{ getElapsedTime(record) }}
              </span>
            </div>
            <div
              v-else-if="record.response_time_ms != null"
              class="flex flex-col items-end text-xs gap-0.5"
            >
              <span
                v-if="record.first_byte_time_ms != null"
                class="tabular-nums"
              >{{ (record.first_byte_time_ms / 1000).toFixed(2) }}s</span>
              <span
                v-else
                class="text-muted-foreground"
              >-</span>
              <span class="text-muted-foreground tabular-nums">{{ (record.response_time_ms / 1000).toFixed(2) }}s</span>
            </div>
            <span
              v-else
              class="text-muted-foreground"
            >-</span>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <!-- 分页控件 -->
    <template #pagination>
      <Pagination
        v-if="totalRecords > 0"
        :current="currentPage"
        :total="totalRecords"
        :page-size="pageSize"
        :page-size-options="pageSizeOptions"
        @update:current="$emit('update:currentPage', $event)"
        @update:page-size="$emit('update:pageSize', $event)"
      />
    </template>
  </TableCard>
</template>

<script setup lang="ts">
import { ref, computed, onUnmounted, watch } from 'vue'
import {
  TableCard,
  Badge,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Pagination,
  RefreshButton,
} from '@/components/ui'
import { formatTokens, formatCurrency } from '@/utils/format'
import { formatDateTime } from '../composables'
import { useRowClick } from '@/composables/useRowClick'
import type { UsageRecord } from '../types'

export interface UserOption {
  id: string
  username: string
  email: string
}

const props = defineProps<{
  records: UsageRecord[]
  isAdmin: boolean
  showActualCost: boolean
  loading: boolean
  // 时间段
  selectedPeriod: string
  // 筛选
  filterUser: string
  filterModel: string
  filterProvider: string
  filterStatus: string
  availableUsers: UserOption[]
  availableModels: string[]
  availableProviders: string[]
  // 分页
  currentPage: number
  pageSize: number
  totalRecords: number
  pageSizeOptions: number[]
}>()

const emit = defineEmits<{
  'update:selectedPeriod': [value: string]
  'update:filterUser': [value: string]
  'update:filterModel': [value: string]
  'update:filterProvider': [value: string]
  'update:filterStatus': [value: string]
  'update:currentPage': [value: number]
  'update:pageSize': [value: number]
  'refresh': []
  'showDetail': [id: string]
}>()

// Select 打开状态
const periodSelectOpen = ref(false)
const filterUserSelectOpen = ref(false)
const filterModelSelectOpen = ref(false)
const filterProviderSelectOpen = ref(false)
const filterStatusSelectOpen = ref(false)

// 动态计时器相关
const now = ref(Date.now())
let timerInterval: ReturnType<typeof setInterval> | null = null

// 检查是否有活跃请求
const hasActiveRecords = computed(() => {
  return props.records.some(r => r.status === 'pending' || r.status === 'streaming')
})

// 启动计时器
function startTimer() {
  if (timerInterval) return
  timerInterval = setInterval(() => {
    now.value = Date.now()
  }, 100) // 每 100ms 更新一次
}

// 停止计时器
function stopTimer() {
  if (timerInterval) {
    clearInterval(timerInterval)
    timerInterval = null
  }
}

// 计算活跃请求的实时耗时
function getElapsedTime(record: UsageRecord): string {
  if (record.status !== 'pending' && record.status !== 'streaming') {
    // 非活跃状态，显示实际响应时间
    if (record.response_time_ms) {
      return `${(record.response_time_ms / 1000).toFixed(2)}s`
    }
    return '-'
  }

  // 活跃状态，计算实时耗时
  if (!record.created_at) return '-'

  const createdAt = new Date(record.created_at).getTime()
  const elapsed = now.value - createdAt

  if (elapsed < 0) return '0.00s'
  return `${(elapsed / 1000).toFixed(2)}s`
}

// 监听活跃记录状态，自动启动/停止计时器
watch(hasActiveRecords, (hasActive) => {
  if (hasActive) {
    startTimer()
  } else {
    stopTimer()
  }
}, { immediate: true })

// 使用复用的行点击逻辑
const { handleMouseDown, shouldTriggerRowClick } = useRowClick()

// 处理行点击，排除文本选择操作
function handleRowClick(event: MouseEvent, id: string) {
  if (!props.isAdmin) return
  if (!shouldTriggerRowClick(event)) return
  emit('showDetail', id)
}

// 组件卸载时清理
onUnmounted(() => {
  stopTimer()
})

// 格式化 API 格式显示名称
function formatApiFormat(format: string): string {
  const formatMap: Record<string, string> = {
    'CLAUDE': 'Claude',
    'CLAUDE_CLI': 'Claude CLI',
    'OPENAI': 'OpenAI',
    'OPENAI_CLI': 'OpenAI CLI',
    'GEMINI': 'Gemini',
    'GEMINI_CLI': 'Gemini CLI',
  }
  return formatMap[format.toUpperCase()] || format
}

// 获取实际使用的模型（优先 target_model，其次 model_version）
// 只有当实际模型与请求模型不同时才返回，用于显示映射箭头
function getActualModel(record: UsageRecord): string | null {
  // 优先显示模型映射
  if (record.target_model && record.target_model !== record.model) {
    return record.target_model
  }
  // 其次显示 Provider 返回的实际版本（如 Gemini 的 modelVersion）
  if (record.request_metadata?.model_version && record.request_metadata.model_version !== record.model) {
    return record.request_metadata.model_version
  }
  return null
}

// 获取模型列的 tooltip
function getModelTooltip(record: UsageRecord): string {
  const actualModel = getActualModel(record)
  if (actualModel) {
    return `${record.model} -> ${actualModel}`
  }
  return record.model
}
</script>
