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

      <!-- 通用搜索 -->
      <div class="relative">
        <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
        <Input
          id="usage-records-search"
          v-model="localSearch"
          :placeholder="isAdmin ? '搜索用户/密钥/模型/提供商' : '搜索密钥/模型'"
          class="w-32 sm:w-48 h-8 text-xs border-border/60 pl-8"
        />
      </div>

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

      <!-- 自动刷新按钮 -->
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        :class="autoRefresh ? 'text-primary' : ''"
        :title="autoRefresh ? '点击关闭自动刷新' : '点击开启自动刷新（每5秒刷新）'"
        @click="$emit('update:autoRefresh', !autoRefresh)"
      >
        <RefreshCcw
          class="w-3.5 h-3.5"
          :class="autoRefresh ? 'animate-spin' : ''"
        />
      </Button>
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
          <TableHead
            v-if="!isAdmin"
            class="h-12 font-semibold w-[100px]"
          >
            密钥
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
          <TableHead class="h-12 font-semibold w-[70px] text-center">
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
            :colspan="isAdmin ? 9 : 8"
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
            <div class="flex flex-col text-xs gap-0.5">
              <span class="truncate">
                {{ record.username || record.user_email || (record.user_id ? `User ${record.user_id}` : '已删除用户') }}
              </span>
              <span
                v-if="record.api_key?.name"
                class="text-muted-foreground truncate"
                :title="record.api_key.name"
              >
                {{ record.api_key.name }}
              </span>
            </div>
          </TableCell>
          <!-- 用户页面的密钥列 -->
          <TableCell
            v-if="!isAdmin"
            class="py-4 w-[100px]"
            :title="record.api_key?.name || '-'"
          >
            <div class="flex flex-col text-xs gap-0.5">
              <span class="truncate">{{ record.api_key?.name || '-' }}</span>
              <span
                v-if="record.api_key?.display"
                class="text-muted-foreground truncate"
              >
                {{ record.api_key.display }}
              </span>
            </div>
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
                <!-- 故障转移图标（优先显示） -->
                <span
                  v-if="record.has_fallback"
                  class="inline-flex items-center justify-center w-4 h-4 text-xs text-amber-600 dark:text-amber-400"
                  title="此请求发生了 Provider 故障转移"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="w-3.5 h-3.5"
                  >
                    <path d="m16 3 4 4-4 4" />
                    <path d="M20 7H4" />
                    <path d="m8 21-4-4 4-4" />
                    <path d="M4 17h16" />
                  </svg>
                </span>
                <!-- 重试图标（仅在无故障转移时显示） -->
                <span
                  v-else-if="record.has_retry"
                  class="inline-flex items-center justify-center w-4 h-4 text-xs text-blue-600 dark:text-blue-400"
                  title="此请求发生了亲和缓存重试"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="w-3.5 h-3.5"
                  >
                    <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
                    <path d="M21 21v-5h-5" />
                    <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
                    <path d="M3 3v5h5" />
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
          <TableCell class="text-center py-4 w-[70px]">
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
            <!-- pending 状态：只显示增长的总时间 -->
            <div
              v-if="record.status === 'pending'"
              class="flex flex-col items-end text-xs gap-0.5"
            >
              <span class="text-muted-foreground">-</span>
              <span class="text-primary tabular-nums">
                {{ getElapsedTime(record) }}
              </span>
            </div>
            <!-- streaming 状态：首字固定 + 总时间增长 -->
            <div
              v-else-if="record.status === 'streaming'"
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
              <span class="text-primary tabular-nums">
                {{ getElapsedTime(record) }}
              </span>
            </div>
            <!-- 已完成状态：首字 + 总耗时 -->
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
        cache-key="usage-records-page-size"
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
  Button,
  Input,
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
} from '@/components/ui'
import { RefreshCcw, Search } from 'lucide-vue-next'
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
  filterSearch: string
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
  // 自动刷新
  autoRefresh: boolean
}>()

const emit = defineEmits<{
  'update:selectedPeriod': [value: string]
  'update:filterSearch': [value: string]
  'update:filterUser': [value: string]
  'update:filterModel': [value: string]
  'update:filterProvider': [value: string]
  'update:filterStatus': [value: string]
  'update:currentPage': [value: number]
  'update:pageSize': [value: number]
  'update:autoRefresh': [value: boolean]
  'refresh': []
  'showDetail': [id: string]
}>()

// Select 打开状态
const periodSelectOpen = ref(false)
const filterUserSelectOpen = ref(false)
const filterModelSelectOpen = ref(false)
const filterProviderSelectOpen = ref(false)
const filterStatusSelectOpen = ref(false)

// 通用搜索（输入防抖）
const localSearch = ref(props.filterSearch)
let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null

watch(() => props.filterSearch, (value) => {
  if (value !== localSearch.value) {
    localSearch.value = value
  }
})

watch(localSearch, (value) => {
  if (searchDebounceTimer) clearTimeout(searchDebounceTimer)
  searchDebounceTimer = setTimeout(() => {
    emit('update:filterSearch', value)
  }, 300)
})

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
  if (searchDebounceTimer) {
    clearTimeout(searchDebounceTimer)
    searchDebounceTimer = null
  }
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
