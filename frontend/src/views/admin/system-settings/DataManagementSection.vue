<template>
  <CardSection
    title="数据管理"
    description="清空系统数据，操作不可逆，请谨慎使用"
  >
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      <div
        v-for="item in purgeItems"
        :key="item.key"
        class="flex flex-col gap-2 p-4 rounded-lg border border-border"
      >
        <div class="flex items-center gap-2">
          <component
            :is="item.icon"
            class="w-4 h-4 text-muted-foreground"
          />
          <span class="text-sm font-medium">{{ item.title }}</span>
        </div>
        <p class="text-xs text-muted-foreground flex-1">
          {{ item.description }}
        </p>

        <div
          v-if="item.key === 'request-bodies'"
          class="flex flex-col gap-1.5"
        >
          <label class="text-xs text-muted-foreground">时间范围</label>
          <Select v-model="requestBodyCutoff" :disabled="isBodyTaskActive">
            <SelectTrigger class="w-full h-8 text-xs">
              <SelectValue placeholder="选择清理范围" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="7">仅清 7 天以前</SelectItem>
              <SelectItem value="30">仅清 30 天以前</SelectItem>
              <SelectItem value="90">仅清 90 天以前</SelectItem>
              <SelectItem value="0">全部清空</SelectItem>
            </SelectContent>
          </Select>
          <p
            v-if="requestBodyProgressText"
            class="text-xs text-muted-foreground mt-1"
          >
            {{ requestBodyProgressText }}
          </p>
        </div>

        <Button
          variant="destructive"
          size="sm"
          class="w-full mt-1"
          :disabled="loadingKey === item.key || (item.key === 'request-bodies' && isBodyTaskActive)"
          @click="handlePurge(item)"
        >
          <Trash2 class="w-3.5 h-3.5 mr-1.5" />
          <template v-if="item.key === 'request-bodies' && isBodyTaskActive">
            清理中…{{ requestBodyTotal }} 条
          </template>
          <template v-else>
            {{ loadingKey === item.key ? '清空中...' : item.buttonText }}
          </template>
        </Button>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { computed, markRaw, onBeforeUnmount, ref, type Component } from 'vue'
import { Trash2, Settings, Users, BarChart3, Shield, FileText, PieChart } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import { CardSection } from '@/components/layout'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui'
import { adminApi, type PurgeRequestBodiesTaskStatus } from '@/api/admin'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'

interface PurgeItem {
  key: string
  title: string
  description: string
  buttonText: string
  icon: Component
  confirmMessage: string
  action: () => Promise<{ message: string } | void>
}

const { success, error, info } = useToast()
const { confirmDanger } = useConfirm()
const loadingKey = ref<string | null>(null)

// 清空请求体专用：时间范围 + 任务轮询状态
const requestBodyCutoff = ref<string>('7')
const requestBodyTaskId = ref<string | null>(null)
const requestBodyTotal = ref<number>(0)
const requestBodyStatus = ref<PurgeRequestBodiesTaskStatus['status'] | null>(null)
const requestBodyError = ref<string | null>(null)
let pollTimer: ReturnType<typeof setInterval> | null = null

const isBodyTaskActive = computed(
  () => requestBodyStatus.value === 'pending' || requestBodyStatus.value === 'running',
)

const requestBodyProgressText = computed(() => {
  if (isBodyTaskActive.value) {
    return `已清理 ${requestBodyTotal.value} 条…`
  }
  if (requestBodyStatus.value === 'completed') {
    return `上次任务已完成，共清理 ${requestBodyTotal.value} 条`
  }
  if (requestBodyStatus.value === 'failed') {
    return `上次任务失败：${requestBodyError.value ?? '未知错误'}`
  }
  return ''
})

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

async function pollOnce() {
  const taskId = requestBodyTaskId.value
  if (!taskId) return
  try {
    const s = await adminApi.getPurgeRequestBodiesStatus(taskId)
    requestBodyStatus.value = s.status
    requestBodyTotal.value = s.total_cleaned ?? 0
    requestBodyError.value = s.error ?? null
    if (s.status === 'completed') {
      stopPolling()
      success(`请求体清理完成，共清理 ${s.total_cleaned ?? 0} 条`)
    } else if (s.status === 'failed') {
      stopPolling()
      error(`请求体清理失败：${s.error ?? '未知错误'}`)
    }
  } catch (e) {
    stopPolling()
    error(parseApiError(e))
  }
}

function startPolling(taskId: string) {
  stopPolling()
  requestBodyTaskId.value = taskId
  pollTimer = setInterval(() => {
    void pollOnce()
  }, 2000)
}

async function handlePurgeRequestBodies() {
  const cutoffDays = Number.parseInt(requestBodyCutoff.value, 10)
  const cutoffLabel = cutoffDays > 0 ? `${cutoffDays} 天以前的` : '全部'
  const confirmed = await confirmDanger(
    `确定要清空${cutoffLabel}请求体吗？请求/响应内容将被清除（含请求/响应头），但 token 和成本等统计信息会保留。清理将在后台分批执行。`,
    '清空请求体',
  )
  if (!confirmed) return

  try {
    const payload = cutoffDays > 0 ? { cutoff_days: cutoffDays } : { cutoff_days: null }
    const resp = await adminApi.purgeRequestBodies(payload)
    requestBodyStatus.value = resp.status
    requestBodyTotal.value = resp.total_cleaned ?? 0
    requestBodyError.value = null
    if (resp.reused) {
      info('已有清理任务在运行，继续跟踪进度')
    } else {
      info('清理任务已启动，将在后台分批执行')
    }
    startPolling(resp.task_id)
  } catch (e) {
    error(parseApiError(e))
  }
}

const purgeItems: PurgeItem[] = [
  {
    key: 'config',
    title: '清空配置',
    description: '删除所有提供商、端点、API Key 和模型配置',
    buttonText: '清空配置',
    icon: markRaw(Settings),
    confirmMessage: '确定要清空所有提供商配置吗？这将删除所有提供商、端点、API Key 和模型配置，操作不可逆。',
    action: () => adminApi.purgeConfig(),
  },
  {
    key: 'users',
    title: '清空用户',
    description: '删除所有非管理员用户及其 API Keys',
    buttonText: '清空用户',
    icon: markRaw(Users),
    confirmMessage: '确定要清空所有非管理员用户吗？管理员账户将被保留，操作不可逆。',
    action: () => adminApi.purgeUsers(),
  },
  {
    key: 'usage',
    title: '清空使用记录',
    description: '删除全部使用记录和请求候选记录',
    buttonText: '清空记录',
    icon: markRaw(BarChart3),
    confirmMessage: '确定要清空全部使用记录吗？所有请求统计数据将被永久删除，操作不可逆。',
    action: () => adminApi.purgeUsage(),
  },
  {
    key: 'audit-logs',
    title: '清空审计日志',
    description: '删除全部审计日志记录',
    buttonText: '清空日志',
    icon: markRaw(Shield),
    confirmMessage: '确定要清空全部审计日志吗？所有安全事件记录将被永久删除，操作不可逆。',
    action: () => adminApi.purgeAuditLogs(),
  },
  {
    key: 'request-bodies',
    title: '清空请求体',
    description: '清空请求/响应体及头部内容（后台分批执行），保留 token 和成本统计',
    buttonText: '启动清理',
    icon: markRaw(FileText),
    confirmMessage: '',
    action: async () => {
      await handlePurgeRequestBodies()
    },
  },
  {
    key: 'stats',
    title: '清空聚合数据',
    description: '清空仪表盘统计和聚合数据，保留原始使用记录',
    buttonText: '清空聚合数据',
    icon: markRaw(PieChart),
    confirmMessage: '确定要清空全部聚合统计数据吗？仪表盘数据将被清除，用户和 Key 的累计统计也会归零，操作不可逆。',
    action: () => adminApi.purgeStats(),
  },
]

async function handlePurge(item: PurgeItem) {
  // 清空请求体走独立流程（带时间范围 + 异步轮询）
  if (item.key === 'request-bodies') {
    await handlePurgeRequestBodies()
    return
  }

  const confirmed = await confirmDanger(item.confirmMessage, item.title)
  if (!confirmed) return

  loadingKey.value = item.key
  try {
    const result = await item.action()
    if (result && typeof result === 'object' && 'message' in result) {
      success((result as { message: string }).message)
    }
  } catch (e) {
    error(parseApiError(e))
  } finally {
    loadingKey.value = null
  }
}

onBeforeUnmount(() => {
  stopPolling()
})
</script>
