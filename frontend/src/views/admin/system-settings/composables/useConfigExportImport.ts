import { nextTick, ref } from 'vue'
import type { AxiosProgressEvent } from 'axios'
import { useToast } from '@/composables/useToast'
import {
  adminApi,
  type AggregateExportData,
  type AggregateImportResponse,
  type ConfigExportData,
  type ConfigImportResponse,
  type UsersExportData,
  type UsersImportResponse,
} from '@/api/admin'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import type { SystemConfig } from './useSystemConfig'

// 文件大小限制：导出文件可能包含大量 Provider Key、模型和用户数据。
const BYTES_PER_MB = 1024 * 1024
const MAX_FILE_SIZE_MB = 500
const MAX_AGGREGATE_FILE_SIZE_MB = 500
const MAX_FILE_SIZE = MAX_FILE_SIZE_MB * BYTES_PER_MB
const MAX_AGGREGATE_FILE_SIZE = MAX_AGGREGATE_FILE_SIZE_MB * BYTES_PER_MB

type JsonObject = Record<string, unknown>

export interface ImportProgressState {
  percent: number
  message: string
}

function asJsonObject(value: unknown): JsonObject | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as JsonObject
    : null
}

function hasArrayField(value: JsonObject, key: string): boolean {
  return Array.isArray(value[key])
}

function looksLikeConfigExport(value: JsonObject): boolean {
  return hasArrayField(value, 'global_models')
    || hasArrayField(value, 'providers')
    || hasArrayField(value, 'proxy_nodes')
    || hasArrayField(value, 'oauth_providers')
    || hasArrayField(value, 'system_configs')
    || Object.prototype.hasOwnProperty.call(value, 'ldap_config')
}

function looksLikeUsersExport(value: JsonObject): boolean {
  return hasArrayField(value, 'users')
    || hasArrayField(value, 'standalone_keys')
    || hasArrayField(value, 'user_groups')
}

function looksLikeAggregateExport(value: JsonObject): boolean {
  return asJsonObject(value.config_data) != null
    && asJsonObject(value.user_data) != null
}

function fileSizeLimitMessage(limitMb: number): string {
  return `文件大小不能超过 ${limitMb}MB`
}

function formatBytes(bytes: number): string {
  if (bytes >= BYTES_PER_MB) {
    return `${(bytes / BYTES_PER_MB).toFixed(1)}MB`
  }
  if (bytes >= 1024) {
    return `${Math.round(bytes / 1024)}KB`
  }
  return `${bytes}B`
}

function setImportProgress(
  target: { value: ImportProgressState | null },
  percent: number,
  message: string,
) {
  target.value = {
    percent: Math.max(0, Math.min(100, Math.round(percent))),
    message,
  }
}

function buildUploadProgressHandler(
  target: { value: ImportProgressState | null },
  label: string,
) {
  return (event: AxiosProgressEvent) => {
    if (!event.total) {
      setImportProgress(target, 15, `${label}上传中：${formatBytes(event.loaded)}`)
      return
    }

    const uploadPercent = Math.min(85, 10 + (event.loaded / event.total) * 75)
    const loaded = formatBytes(event.loaded)
    const total = formatBytes(event.total)
    const message = event.loaded >= event.total
      ? `${label}已上传，服务端正在校验并写入数据`
      : `${label}上传中：${loaded} / ${total}`
    setImportProgress(target, uploadPercent, message)
  }
}

function downloadJson(data: unknown, filename: string) {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

export function useConfigExportImport(systemConfig: { value: SystemConfig }) {
  const { success, error } = useToast()

  // 配置导出/导入相关
  const exportLoading = ref(false)
  const importLoading = ref(false)
  const importDialogOpen = ref(false)
  const importResultDialogOpen = ref(false)
  const configFileInput = ref<HTMLInputElement | null>(null)
  const importPreview = ref<ConfigExportData | null>(null)
  const importResult = ref<ConfigImportResponse | null>(null)
  const mergeMode = ref<'skip' | 'overwrite' | 'error'>('skip')
  const mergeModeSelectOpen = ref(false)
  const importProgress = ref<ImportProgressState | null>(null)

  // 用户数据导出/导入相关
  const exportUsersLoading = ref(false)
  const importUsersLoading = ref(false)
  const importUsersDialogOpen = ref(false)
  const importUsersResultDialogOpen = ref(false)
  const usersFileInput = ref<HTMLInputElement | null>(null)
  const importUsersPreview = ref<UsersExportData | null>(null)
  const importUsersResult = ref<UsersImportResponse | null>(null)
  const usersMergeMode = ref<'skip' | 'overwrite' | 'error'>('skip')
  const usersMergeModeSelectOpen = ref(false)
  const importUsersProgress = ref<ImportProgressState | null>(null)

  // 完整备份导出/导入相关
  const exportAggregateLoading = ref(false)
  const importAggregateLoading = ref(false)
  const aggregateImportDialogOpen = ref(false)
  const aggregateImportResultDialogOpen = ref(false)
  const aggregateImportPreview = ref<AggregateExportData | null>(null)
  const aggregateImportResult = ref<AggregateImportResponse | null>(null)
  const aggregateMergeMode = ref<'skip' | 'overwrite' | 'error'>('skip')
  const aggregateMergeModeSelectOpen = ref(false)
  const importAggregateProgress = ref<ImportProgressState | null>(null)

  // 导出配置
  async function handleExportConfig() {
    exportLoading.value = true
    try {
      const data = await adminApi.exportConfig()
      downloadJson(
        data,
        `${systemConfig.value.site_name.toLowerCase()}-config-${new Date().toISOString().slice(0, 10)}.json`,
      )
      success('配置已导出')
    } catch (err) {
      error('导出配置失败')
      log.error('导出配置失败:', err)
    } finally {
      exportLoading.value = false
    }
  }

  // 触发文件选择
  function triggerConfigFileSelect() {
    configFileInput.value?.click()
  }

  // 处理文件选择
  function handleConfigFileSelect(event: Event) {
    const input = event.target as HTMLInputElement
    const file = input.files?.[0]
    if (!file) return

    if (file.size > MAX_FILE_SIZE) {
      error(fileSizeLimitMessage(MAX_FILE_SIZE_MB))
      input.value = ''
      return
    }

    const reader = new FileReader()
    reader.onload = (e) => {
      try {
        const content = e.target?.result as string
        const root = asJsonObject(JSON.parse(content))
        if (!root) {
          error('无效的配置文件：JSON 顶层必须是对象')
          return
        }

        if (looksLikeUsersExport(root) && !looksLikeConfigExport(root)) {
          error('这是用户数据导出文件，请使用“导入用户数据”')
          return
        }

        if (!root.version) {
          error('无效的配置文件：缺少版本信息')
          return
        }

        if (!looksLikeConfigExport(root)) {
          error('无效的配置文件：未找到配置导出内容')
          return
        }

        const data = root as unknown as ConfigExportData
        importPreview.value = data
        mergeMode.value = 'skip'
        importDialogOpen.value = true
      } catch (err) {
        error('解析配置文件失败，请确保是有效的 JSON 文件')
        log.error('解析配置文件失败:', err)
      }
    }
    reader.readAsText(file)

    input.value = ''
  }

  // 确认导入
  async function confirmImport() {
    if (!importPreview.value) return

    importLoading.value = true
    setImportProgress(importProgress, 5, '准备提交配置数据')
    await nextTick()
    try {
      const result = await adminApi.importConfig({
        ...importPreview.value,
        merge_mode: mergeMode.value,
      }, {
        onUploadProgress: buildUploadProgressHandler(importProgress, '配置数据'),
      })
      setImportProgress(importProgress, 100, '配置数据导入完成')
      importResult.value = result
      importDialogOpen.value = false
      mergeModeSelectOpen.value = false
      importResultDialogOpen.value = true
      success('配置导入成功')
    } catch (err: unknown) {
      error(parseApiError(err, '导入配置失败'))
      log.error('导入配置失败:', err)
    } finally {
      importLoading.value = false
      importProgress.value = null
    }
  }

  // 导出用户数据
  async function handleExportUsers() {
    exportUsersLoading.value = true
    try {
      const data = await adminApi.exportUsers()
      downloadJson(
        data,
        `${systemConfig.value.site_name.toLowerCase()}-users-${new Date().toISOString().slice(0, 10)}.json`,
      )
      success('用户数据已导出')
    } catch (err) {
      error('导出用户数据失败')
      log.error('导出用户数据失败:', err)
    } finally {
      exportUsersLoading.value = false
    }
  }

  // 触发用户数据文件选择
  function triggerUsersFileSelect() {
    usersFileInput.value?.click()
  }

  // 处理用户数据文件选择
  function handleUsersFileSelect(event: Event) {
    const input = event.target as HTMLInputElement
    const file = input.files?.[0]
    if (!file) return

    if (file.size > MAX_FILE_SIZE) {
      error(fileSizeLimitMessage(MAX_FILE_SIZE_MB))
      input.value = ''
      return
    }

    const reader = new FileReader()
    reader.onload = (e) => {
      try {
        const content = e.target?.result as string
        const root = asJsonObject(JSON.parse(content))
        if (!root) {
          error('无效的用户数据文件：JSON 顶层必须是对象')
          return
        }

        if (looksLikeConfigExport(root) && !looksLikeUsersExport(root)) {
          error('这是配置导出文件，请使用“导入配置”')
          return
        }

        if (!root.version) {
          error('无效的用户数据文件：缺少版本信息')
          return
        }

        if (!Array.isArray(root.users)) {
          error('无效的用户数据文件：缺少 users 数组')
          return
        }

        if (root.user_groups != null && !Array.isArray(root.user_groups)) {
          error('无效的用户数据文件：user_groups 必须是数组')
          return
        }

        if (root.standalone_keys != null && !Array.isArray(root.standalone_keys)) {
          error('无效的用户数据文件：standalone_keys 必须是数组')
          return
        }

        const data = root as unknown as UsersExportData
        importUsersPreview.value = data
        usersMergeMode.value = 'skip'
        importUsersDialogOpen.value = true
      } catch (err) {
        error('解析用户数据文件失败，请确保是有效的 JSON 文件')
        log.error('解析用户数据文件失败:', err)
      }
    }
    reader.readAsText(file)

    input.value = ''
  }

  // 确认导入用户数据
  async function confirmImportUsers() {
    if (!importUsersPreview.value) return

    importUsersLoading.value = true
    setImportProgress(importUsersProgress, 5, '准备提交用户数据')
    await nextTick()
    try {
      const result = await adminApi.importUsers({
        ...importUsersPreview.value,
        merge_mode: usersMergeMode.value,
      }, {
        onUploadProgress: buildUploadProgressHandler(importUsersProgress, '用户数据'),
      })
      setImportProgress(importUsersProgress, 100, '用户数据导入完成')
      importUsersResult.value = result
      importUsersDialogOpen.value = false
      usersMergeModeSelectOpen.value = false
      importUsersResultDialogOpen.value = true
      success('用户数据导入成功')
    } catch (err: unknown) {
      error(parseApiError(err, '导入用户数据失败'))
      log.error('导入用户数据失败:', err)
    } finally {
      importUsersLoading.value = false
      importUsersProgress.value = null
    }
  }

  // 导出完整备份
  async function handleExportAggregate() {
    exportAggregateLoading.value = true
    try {
      const data = await adminApi.exportAggregateData()
      downloadJson(
        data,
        `${systemConfig.value.site_name.toLowerCase()}-data-${new Date().toISOString().slice(0, 10)}.json`,
      )
      success('完整备份已导出')
    } catch (err) {
      error('导出完整备份失败')
      log.error('导出完整备份失败:', err)
    } finally {
      exportAggregateLoading.value = false
    }
  }

  // 处理完整备份文件选择
  function handleAggregateFileSelect(event: Event) {
    const input = event.target as HTMLInputElement
    const file = input.files?.[0]
    if (!file) return

    if (file.size > MAX_AGGREGATE_FILE_SIZE) {
      error(fileSizeLimitMessage(MAX_AGGREGATE_FILE_SIZE_MB))
      input.value = ''
      return
    }

    const reader = new FileReader()
    reader.onload = (e) => {
      try {
        const content = e.target?.result as string
        const root = asJsonObject(JSON.parse(content))
        if (!root) {
          error('无效的完整备份文件：JSON 顶层必须是对象')
          return
        }

        if (!looksLikeAggregateExport(root)) {
          if (looksLikeConfigExport(root)) {
            error('这是配置导出文件，请使用“导入配置数据”')
          } else if (looksLikeUsersExport(root)) {
            error('这是用户数据导出文件，请使用“导入用户数据”')
          } else {
            error('无效的完整备份文件：未找到配置数据和用户数据')
          }
          return
        }

        if (!root.version) {
          error('无效的完整备份文件：缺少版本信息')
          return
        }

        const configData = asJsonObject(root.config_data)
        const userData = asJsonObject(root.user_data)
        if (!configData || !looksLikeConfigExport(configData)) {
          error('无效的完整备份文件：config_data 格式不正确')
          return
        }
        if (!userData || !looksLikeUsersExport(userData)) {
          error('无效的完整备份文件：user_data 格式不正确')
          return
        }

        const data = root as unknown as AggregateExportData
        aggregateImportPreview.value = data
        aggregateMergeMode.value = 'skip'
        aggregateImportDialogOpen.value = true
      } catch (err) {
        error('解析完整备份文件失败，请确保是有效的 JSON 文件')
        log.error('解析完整备份文件失败:', err)
      }
    }
    reader.readAsText(file)

    input.value = ''
  }

  // 确认导入完整备份
  async function confirmImportAggregate() {
    if (!aggregateImportPreview.value) return

    importAggregateLoading.value = true
    setImportProgress(importAggregateProgress, 5, '准备提交完整备份')
    await nextTick()
    try {
      const result = await adminApi.importAggregateData({
        ...aggregateImportPreview.value,
        merge_mode: aggregateMergeMode.value,
      }, {
        onUploadProgress: buildUploadProgressHandler(importAggregateProgress, '完整备份'),
      })
      setImportProgress(importAggregateProgress, 100, '完整备份导入完成')
      aggregateImportResult.value = result
      aggregateImportDialogOpen.value = false
      aggregateMergeModeSelectOpen.value = false
      aggregateImportResultDialogOpen.value = true
      success('完整备份导入成功')
    } catch (err: unknown) {
      error(parseApiError(err, '导入完整备份失败'))
      log.error('导入完整备份失败:', err)
    } finally {
      importAggregateLoading.value = false
      importAggregateProgress.value = null
    }
  }

  return {
    // 配置导出/导入
    exportLoading,
    importLoading,
    importDialogOpen,
    importResultDialogOpen,
    configFileInput,
    importPreview,
    importResult,
    mergeMode,
    mergeModeSelectOpen,
    importProgress,
    handleExportConfig,
    triggerConfigFileSelect,
    handleConfigFileSelect,
    confirmImport,
    // 用户数据导出/导入
    exportUsersLoading,
    importUsersLoading,
    importUsersDialogOpen,
    importUsersResultDialogOpen,
    usersFileInput,
    importUsersPreview,
    importUsersResult,
    usersMergeMode,
    usersMergeModeSelectOpen,
    importUsersProgress,
    handleExportUsers,
    triggerUsersFileSelect,
    handleUsersFileSelect,
    confirmImportUsers,
    // 完整备份导出/导入
    exportAggregateLoading,
    importAggregateLoading,
    aggregateImportDialogOpen,
    aggregateImportResultDialogOpen,
    aggregateImportPreview,
    aggregateImportResult,
    aggregateMergeMode,
    aggregateMergeModeSelectOpen,
    importAggregateProgress,
    handleExportAggregate,
    handleAggregateFileSelect,
    confirmImportAggregate,
  }
}
