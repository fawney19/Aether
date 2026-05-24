<template>
  <CardSection
    title="数据管理"
    description="导出、导入或清空系统数据"
  >
    <div class="space-y-6">
      <div>
        <div class="flex items-center gap-2 mb-3">
          <Database class="w-4 h-4 text-muted-foreground" />
          <h4 class="text-sm font-medium">
            导出 / 导入
          </h4>
        </div>

        <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
          <div
            v-for="item in dataItems"
            :key="item.key"
            class="flex flex-col gap-3 p-4 rounded-lg border border-border"
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
            <div class="grid grid-cols-2 gap-2">
              <Button
                variant="outline"
                size="sm"
                class="w-full"
                :disabled="item.exportLoading"
                @click="$emit('export', item.key)"
              >
                <Download class="w-3.5 h-3.5 mr-1.5" />
                {{ item.exportLoading ? '导出中...' : item.exportLabel }}
              </Button>
              <Button
                variant="outline"
                size="sm"
                class="w-full"
                :disabled="item.importLoading"
                @click="triggerDataFileSelect(item.key)"
              >
                <Upload class="w-3.5 h-3.5 mr-1.5" />
                {{ item.importLoading ? '导入中...' : item.importLabel }}
              </Button>
            </div>
          </div>
        </div>

        <input
          ref="configFileInput"
          type="file"
          accept=".json"
          class="hidden"
          @change="$emit('fileSelect', 'config', $event)"
        >
        <input
          ref="usersFileInput"
          type="file"
          accept=".json"
          class="hidden"
          @change="$emit('fileSelect', 'users', $event)"
        >
        <input
          ref="aggregateFileInput"
          type="file"
          accept=".json"
          class="hidden"
          @change="$emit('fileSelect', 'aggregate', $event)"
        >
      </div>

      <Separator />

      <div>
        <div class="flex items-center gap-2 mb-3">
          <Trash2 class="w-4 h-4 text-muted-foreground" />
          <h4 class="text-sm font-medium">
            清空数据
          </h4>
        </div>

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
            <Button
              variant="destructive"
              size="sm"
              class="w-full mt-1"
              :disabled="loadingKey === item.key"
              @click="handlePurge(item)"
            >
              <Trash2 class="w-3.5 h-3.5 mr-1.5" />
              {{ loadingKey === item.key ? '清空中...' : item.buttonText }}
            </Button>
          </div>
        </div>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { computed, ref, markRaw, type Component } from 'vue'
import {
  Download,
  Upload,
  Settings,
  Users,
  Database,
  Layers3,
  Trash2,
  BarChart3,
  Shield,
  FileText,
  PieChart,
} from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import { Separator } from '@/components/ui'
import { CardSection } from '@/components/layout'
import { adminApi } from '@/api/admin'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'

type DataItemKey = 'config' | 'users' | 'aggregate'

interface DataItem {
  key: DataItemKey
  title: string
  description: string
  exportLabel: string
  importLabel: string
  icon: Component
  exportLoading: boolean
  importLoading: boolean
}

interface PurgeItem {
  key: string
  title: string
  description: string
  buttonText: string
  icon: Component
  confirmMessage: string
  action: () => Promise<{ message: string }>
}

const props = defineProps<{
  configExportLoading: boolean
  configImportLoading: boolean
  usersExportLoading: boolean
  usersImportLoading: boolean
  aggregateExportLoading: boolean
  aggregateImportLoading: boolean
}>()

defineEmits<{
  export: [key: DataItemKey]
  fileSelect: [key: DataItemKey, event: Event]
}>()

const { success, error } = useToast()
const { confirmDanger } = useConfirm()
const loadingKey = ref<string | null>(null)
const configFileInput = ref<HTMLInputElement | null>(null)
const usersFileInput = ref<HTMLInputElement | null>(null)
const aggregateFileInput = ref<HTMLInputElement | null>(null)

const dataItems = computed<DataItem[]>(() => [
  {
    key: 'config',
    title: '配置数据',
    description: '提供商、端点、API Key、模型与系统配置',
    exportLabel: '导出配置',
    importLabel: '导入配置',
    icon: markRaw(Settings),
    exportLoading: props.configExportLoading,
    importLoading: props.configImportLoading,
  },
  {
    key: 'users',
    title: '用户数据',
    description: '普通用户、用户组、API Keys 与钱包快照（不含管理员）',
    exportLabel: '导出用户',
    importLabel: '导入用户',
    icon: markRaw(Users),
    exportLoading: props.usersExportLoading,
    importLoading: props.usersImportLoading,
  },
  {
    key: 'aggregate',
    title: '完整备份',
    description: '配置与用户数据的一体化备份，包含用户、用户组、API Keys 与钱包快照',
    exportLabel: '导出备份',
    importLabel: '导入备份',
    icon: markRaw(Layers3),
    exportLoading: props.aggregateExportLoading,
    importLoading: props.aggregateImportLoading,
  },
])

const purgeItems: PurgeItem[] = [
  {
    key: 'config',
    title: '清空配置',
    description: '后台删除所有提供商、端点、API Key 和模型配置',
    buttonText: '清空配置',
    icon: markRaw(Settings),
    confirmMessage: '确定要后台清空所有提供商配置吗？这将删除所有提供商、端点、API Key 和模型配置，操作不可逆。',
    action: () => adminApi.purgeConfig(),
  },
  {
    key: 'users',
    title: '清空用户',
    description: '后台删除所有非管理员用户及其 API Keys',
    buttonText: '清空用户',
    icon: markRaw(Users),
    confirmMessage: '确定要后台清空所有非管理员用户吗？管理员账户将被保留，操作不可逆。',
    action: () => adminApi.purgeUsers(),
  },
  {
    key: 'usage',
    title: '清空使用记录',
    description: '后台清空全部使用记录和请求候选记录',
    buttonText: '清空记录',
    icon: markRaw(BarChart3),
    confirmMessage: '确定要后台清空全部使用记录吗？所有请求统计数据将被永久删除，操作不可逆。',
    action: () => adminApi.purgeUsage(),
  },
  {
    key: 'audit-logs',
    title: '清空审计日志',
    description: '后台删除全部审计日志记录',
    buttonText: '清空日志',
    icon: markRaw(Shield),
    confirmMessage: '确定要后台清空全部审计日志吗？所有安全事件记录将被永久删除，操作不可逆。',
    action: () => adminApi.purgeAuditLogs(),
  },
  {
    key: 'request-bodies',
    title: '清空请求体',
    description: '后台分批清空所有请求/响应体数据，保留统计信息',
    buttonText: '清空请求体',
    icon: markRaw(FileText),
    confirmMessage: '确定要后台清空全部请求体吗？请求/响应内容将被分批清除，但 token 和成本等统计信息会保留，操作不可逆。',
    action: () => adminApi.purgeRequestBodiesAsync(),
  },
  {
    key: 'stats',
    title: '清空统计聚合',
    description: '后台删除统计聚合数据并重建，保留原始使用记录',
    buttonText: '清空统计聚合',
    icon: markRaw(PieChart),
    confirmMessage: '确定要后台清空全部统计聚合数据吗？原始使用记录会保留，仪表盘和累计统计会从原始记录重新构建。',
    action: () => adminApi.purgeStats(),
  },
]

function triggerDataFileSelect(key: DataItemKey) {
  if (key === 'config') {
    configFileInput.value?.click()
  } else if (key === 'users') {
    usersFileInput.value?.click()
  } else {
    aggregateFileInput.value?.click()
  }
}

async function handlePurge(item: PurgeItem) {
  const confirmed = await confirmDanger(item.confirmMessage, item.title)
  if (!confirmed) return

  loadingKey.value = item.key
  try {
    const result = await item.action()
    success(result.message || '操作成功')
  } catch (e) {
    error(parseApiError(e, '清空失败'))
  } finally {
    loadingKey.value = null
  }
}
</script>
