<template>
  <Dialog
    :model-value="modelValue"
    :title="dialogTitle"
    :description="dialogDescription"
    size="3xl"
    persistent
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="max-h-[calc(100dvh-13rem)] space-y-4 overflow-y-auto overscroll-contain pr-1 sm:max-h-[min(72vh,44rem)] sm:pr-2">
      <div class="space-y-2 rounded-lg border bg-muted/20 px-3 py-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="text-xs font-medium text-foreground">
            操作范围
          </div>
          <Badge
            variant="outline"
            class="text-[11px]"
          >
            {{ selectAllFiltered ? '全选筛选结果' : '表格手动多选' }}
          </Badge>
        </div>
        <div class="text-sm text-foreground">
          已选择 <span class="font-semibold tabular-nums">{{ selectedCount }}</span> 个账号
        </div>
        <div class="text-[11px] text-muted-foreground">
          选择范围来自号池管理表格当前筛选条件
        </div>
      </div>

      <div class="space-y-2 rounded-lg border bg-background px-3 py-3">
        <div>
          <div class="text-xs font-medium text-muted-foreground">
            执行动作
          </div>
          <div class="mt-1 text-sm font-semibold text-foreground">
            {{ selectedActionOption.label }}
          </div>
          <p class="mt-1 text-[11px] text-muted-foreground">
            {{ selectedActionOption.hint }}
          </p>
        </div>

        <div
          v-if="selectedAction === 'set_proxy'"
          class="space-y-2 border-t border-border/60 pt-3"
        >
          <div class="text-[11px] text-muted-foreground">
            选择要绑定的代理节点
          </div>
          <ProxyNodeSelect
            :model-value="proxyNodeIdForAction"
            trigger-class="h-9"
            @update:model-value="(v: string) => proxyNodeIdForAction = v"
          />
        </div>
      </div>

      <section
        v-if="selectedAction === 'update_settings'"
        class="space-y-3 rounded-lg border border-primary/25 bg-primary/5 p-3 sm:p-4"
      >
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div>
            <h3 class="text-sm font-semibold">
              更多设置
            </h3>
            <p class="text-[11px] text-muted-foreground">
              仅更新已勾选字段，未勾选配置保持不变
            </p>
          </div>
          <Badge
            variant="outline"
            class="tabular-nums"
          >
            已选 {{ selectedSettingsCount }} 项
          </Badge>
        </div>

        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          <div class="space-y-2 rounded-md border bg-background p-3">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.internal_priority"
                @update:checked="settingsSelection.internal_priority = $event === true"
              />
              <Label class="text-xs">优先级</Label>
            </div>
            <Input
              v-model.number="settingsDraft.internal_priority"
              type="number"
              min="0"
              class="h-9"
              :disabled="!settingsSelection.internal_priority"
            />
          </div>

          <div class="space-y-2 rounded-md border bg-background p-3">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.rpm_limit"
                @update:checked="settingsSelection.rpm_limit = $event === true"
              />
              <Label class="text-xs">RPM 限制</Label>
            </div>
            <Input
              :model-value="settingsDraft.rpm_limit ?? ''"
              type="number"
              min="1"
              max="10000"
              class="h-9"
              placeholder="留空为自适应"
              :disabled="!settingsSelection.rpm_limit"
              @update:model-value="settingsDraft.rpm_limit = parseNullableNumberInput($event, { min: 1, max: 10000 })"
            />
          </div>

          <div class="space-y-2 rounded-md border bg-background p-3">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.concurrent_limit"
                @update:checked="settingsSelection.concurrent_limit = $event === true"
              />
              <Label class="text-xs">并发请求上限</Label>
            </div>
            <Input
              :model-value="settingsDraft.concurrent_limit ?? ''"
              type="number"
              min="0"
              class="h-9"
              placeholder="留空为不限制"
              :disabled="!settingsSelection.concurrent_limit"
              @update:model-value="settingsDraft.concurrent_limit = parseNullableNumberInput($event, { min: 0 })"
            />
          </div>

          <div class="space-y-2 rounded-md border bg-background p-3">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.cache_ttl_minutes"
                @update:checked="settingsSelection.cache_ttl_minutes = $event === true"
              />
              <Label class="text-xs">缓存 TTL（分钟）</Label>
            </div>
            <Input
              :model-value="settingsDraft.cache_ttl_minutes"
              type="number"
              min="0"
              max="60"
              class="h-9"
              :disabled="!settingsSelection.cache_ttl_minutes"
              @update:model-value="settingsDraft.cache_ttl_minutes = parseNumberInput($event, { min: 0, max: 60 }) ?? 5"
            />
          </div>

          <div class="space-y-2 rounded-md border bg-background p-3">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.max_probe_interval_minutes"
                @update:checked="settingsSelection.max_probe_interval_minutes = $event === true"
              />
              <Label class="text-xs">熔断探测（分钟）</Label>
            </div>
            <Input
              :model-value="settingsDraft.max_probe_interval_minutes"
              type="number"
              min="0"
              max="32"
              class="h-9"
              :disabled="!settingsSelection.max_probe_interval_minutes"
              @update:model-value="settingsDraft.max_probe_interval_minutes = parseNumberInput($event, { min: 0, max: 32 }) ?? 32"
            />
          </div>

          <div class="space-y-2 rounded-md border bg-background p-3">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.is_active"
                @update:checked="settingsSelection.is_active = $event === true"
              />
              <Label class="text-xs">启用状态</Label>
            </div>
            <Select
              v-model="settingsStatus"
              :disabled="!settingsSelection.is_active"
            >
              <SelectTrigger class="h-9">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="enabled">
                  启用
                </SelectItem>
                <SelectItem value="disabled">
                  停用
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div class="space-y-2 rounded-md border bg-background p-3 sm:col-span-2">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.proxy_node_id"
                @update:checked="settingsSelection.proxy_node_id = $event === true"
              />
              <Label class="text-xs">账号代理</Label>
            </div>
            <div class="grid gap-2 sm:grid-cols-[9rem_minmax(0,1fr)]">
              <Select
                v-model="settingsDraft.proxy_mode"
                :disabled="!settingsSelection.proxy_node_id"
              >
                <SelectTrigger class="h-9">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="set">
                    设置节点
                  </SelectItem>
                  <SelectItem value="clear">
                    清除代理
                  </SelectItem>
                </SelectContent>
              </Select>
              <ProxyNodeSelect
                v-if="settingsDraft.proxy_mode === 'set'"
                :model-value="settingsDraft.proxy_node_id"
                trigger-class="h-9"
                :class="!settingsSelection.proxy_node_id ? 'pointer-events-none opacity-50' : ''"
                @update:model-value="(value: string) => settingsDraft.proxy_node_id = value"
              />
              <div
                v-else
                class="flex h-9 items-center rounded-md border bg-muted/30 px-3 text-xs text-muted-foreground"
              >
                回退到 Provider 默认代理
              </div>
            </div>
          </div>

          <div class="space-y-2 rounded-md border bg-background p-3">
            <div class="flex min-h-6 items-center gap-2">
              <Checkbox
                :checked="settingsSelection.note"
                @update:checked="settingsSelection.note = $event === true"
              />
              <Label class="text-xs">备注</Label>
            </div>
            <Input
              v-model="settingsDraft.note"
              class="h-9"
              placeholder="留空清除备注"
              :disabled="!settingsSelection.note"
            />
          </div>
        </div>

        <p class="min-h-5 text-xs text-destructive">
          {{ settingsErrors[0] || '' }}
        </p>
      </section>

      <div
        v-if="executing && progressTotal > 0"
        class="space-y-1"
      >
        <div class="flex items-center justify-between text-xs text-muted-foreground">
          <span>{{ progressLabel }}</span>
          <span>{{ progressDone }} / {{ progressTotal }}</span>
        </div>
        <div class="h-1.5 w-full rounded-full bg-muted overflow-hidden">
          <div
            class="h-full rounded-full bg-primary transition-all duration-150"
            :style="{ width: `${Math.round((progressDone / progressTotal) * 100)}%` }"
          />
        </div>
      </div>
      <div
        v-else-if="lastResultMessage"
        class="rounded-md border bg-background px-3 py-2 text-xs text-muted-foreground"
      >
        {{ lastResultMessage }}
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="executing"
        @click="emit('update:modelValue', false)"
      >
        关闭
      </Button>
      <Button
        :variant="selectedActionOption.destructive ? 'destructive' : 'default'"
        :disabled="!canExecuteSpecifiedAction(selectedAction)"
        @click="confirmAndExecuteAction(selectedAction)"
      >
        {{ executeActionButtonLabel }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue'
import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import ProxyNodeSelect from '@/features/providers/components/ProxyNodeSelect.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import {
  batchActionPoolKeys,
  getPoolBatchDeleteTask,
  resolvePoolKeySelection,
  type PoolKeyDetail,
  type PoolKeySelectionRequest,
  type PoolKeySelectionItem,
} from '@/api/endpoints/pool'
import { exportKey, refreshProviderQuota } from '@/api/endpoints/keys'
import { refreshProviderOAuth } from '@/api/endpoints/provider_oauth'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import {
  canExportOAuthCredential,
  canRefreshOAuthCredential,
} from '@/utils/providerKeyAuth'
import { runChunkedBatchAction } from '@/utils/batchAction'
import { parseNullableNumberInput, parseNumberInput } from '@/utils/form'
import {
  buildPoolKeySettingsPatch,
  createPoolKeyBatchSettingSelection,
  createPoolKeyBatchSettingsDraft,
  validatePoolKeyBatchSettings,
} from '@/features/pool/utils/poolKeyBatchSettings'
import {
  POOL_BATCH_ACTION_OPTIONS,
  type PoolBatchActionValue,
} from '@/features/pool/utils/poolBatchActions'

const props = defineProps<{
  modelValue: boolean
  providerId: string
  providerName?: string
  providerType?: string
  batchConcurrency?: number | null
  selectedKeys: PoolKeyDetail[]
  selectAllFiltered: boolean
  selectedCount: number
  selectionFilters: PoolKeySelectionRequest
  initialAction?: PoolBatchActionValue | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  changed: []
  'edit-config': [keyIds: string[]]
}>()

const ACTION_OPTIONS = POOL_BATCH_ACTION_OPTIONS

const { success, warning, error: showError } = useToast()
const { confirm } = useConfirm()
const proxyNodesStore = useProxyNodesStore()

const executing = ref(false)
const selectedAction = ref<PoolBatchActionValue>('refresh_quota')
const proxyNodeIdForAction = ref('')
const settingsSelection = reactive(createPoolKeyBatchSettingSelection())
const settingsDraft = reactive(createPoolKeyBatchSettingsDraft())
const settingsStatus = computed({
  get: () => settingsDraft.is_active ? 'enabled' : 'disabled',
  set: (value: string) => { settingsDraft.is_active = value === 'enabled' },
})
const lastResultMessage = ref('')
const progressTotal = ref(0)
const progressDone = ref(0)
const progressLabel = ref('')

const dialogDescription = computed(() => {
  const name = (props.providerName || '').trim()
  return name ? `${name} - 对表格选择批量执行动作` : '对表格选择批量执行动作'
})

const selectedCount = computed(() => Math.max(0, Number(props.selectedCount || 0)))
const selectAllFiltered = computed(() => props.selectAllFiltered)
const settingsErrors = computed(() => validatePoolKeyBatchSettings(settingsSelection, settingsDraft))
const selectedSettingsCount = computed(() => Object.values(settingsSelection).filter(Boolean).length)
const selectedActionOption = computed(() => (
  ACTION_OPTIONS.find(option => option.value === selectedAction.value)
  || {
    value: 'refresh_quota' as const,
    label: '刷新额度',
    hint: '调用额度刷新接口，适合核对最新配额状态。',
  }
))
const dialogTitle = computed(() => `执行动作 · ${selectedActionOption.value.label}`)
const executeActionButtonLabel = computed(() => {
  if (executing.value) return '执行中...'
  if (selectedAction.value === 'edit_config') return '编辑配置'
  if (selectedAction.value === 'set_proxy') return '应用代理设置'
  if (selectedAction.value === 'update_settings') return '应用更多设置'
  return `执行${selectedActionOption.value.label}`
})

function sanitizeFileNamePart(value: unknown, fallback: string): string {
  const sanitized = String(value || '')
    .trim()
    .replace(/[^a-zA-Z0-9_\-@.]/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_+|_+$/g, '')
  return sanitized || fallback
}

function formatExportTimestamp(date: Date = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}_${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`
}

function getBatchExportFilename(): string {
  const providerType = sanitizeFileNamePart(props.providerType || 'pool', 'pool')
  const providerName = sanitizeFileNamePart(props.providerName || props.providerId.slice(0, 8), 'provider')
  return `aether_${providerType}_${providerName}_batch_export_${formatExportTimestamp()}.json`
}

function downloadJsonFile(data: unknown, filename: string): void {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  document.body.appendChild(link)
  link.click()
  document.body.removeChild(link)
  URL.revokeObjectURL(url)
}

function canExecuteSpecifiedAction(action: PoolBatchActionValue): boolean {
  if (executing.value || selectedCount.value === 0) return false
  if (action === 'set_proxy') return Boolean(proxyNodeIdForAction.value)
  if (action === 'update_settings') return settingsErrors.value.length === 0
  return true
}

function handleActionButtonClick(action: PoolBatchActionValue): void {
  if (action === 'set_proxy' || action === 'update_settings') {
    selectedAction.value = action
    return
  }
  void confirmAndExecuteAction(action)
}

async function confirmAndExecuteAction(action: PoolBatchActionValue): Promise<void> {
  selectedAction.value = action
  if (selectedCount.value === 0) {
    warning('请先选择账号')
    return
  }
  if (action === 'set_proxy' && !proxyNodeIdForAction.value) {
    warning('请先选择代理节点')
    return
  }
  if (action === 'update_settings' && settingsErrors.value.length > 0) {
    warning(settingsErrors.value[0])
    return
  }
  if (!canExecuteSpecifiedAction(action)) return

  if (action === 'edit_config') {
    await openBatchEditor()
    return
  }

  const actionOption = ACTION_OPTIONS.find((item) => item.value === action)
  const actionLabel = actionOption?.label || '执行动作'
  const scopeLabel = selectAllFiltered.value ? '筛选结果' : '已选账号'
  const confirmed = await confirm({
    title: actionLabel,
    message: `将对${scopeLabel}（${selectedCount.value} 个）执行：${actionLabel}，是否继续？`,
    confirmText: actionOption?.destructive ? '确认删除' : '确认执行',
    ...(actionOption?.destructive ? { variant: 'destructive' as const } : {}),
  })
  if (!confirmed) return
  await executeAction(action)
}

async function openBatchEditor(): Promise<void> {
  if (executing.value || selectedCount.value === 0) return
  executing.value = true
  progressDone.value = 0
  progressTotal.value = 0
  progressLabel.value = selectAllFiltered.value ? '正在解析筛选结果...' : '正在准备批量编辑...'
  try {
    const selectedKeys = await resolveSelectedItems()
    const keyIds = selectedKeys.map(key => key.key_id)
    if (keyIds.length === 0) {
      warning('未找到可编辑账号，请刷新列表重试')
      return
    }
    emit('update:modelValue', false)
    emit('edit-config', keyIds)
  } catch (err) {
    showError(parseApiError(err, '准备批量编辑失败'))
  } finally {
    executing.value = false
    progressDone.value = 0
    progressTotal.value = 0
    progressLabel.value = ''
  }
}

const DELETE_POLL_INTERVAL_MS = 2000
const DELETE_POLL_MAX_MS = 10 * 60 * 1000
const DELETE_POLL_MAX_FAILURES = 3

async function pollDeleteTask(
  providerId: string,
  taskId: string,
  progressOffset: number,
): Promise<{ status: string; deleted: number }> {
  const deadline = Date.now() + DELETE_POLL_MAX_MS
  let consecutiveFailures = 0
  while (Date.now() < deadline) {
    try {
      const task = await getPoolBatchDeleteTask(providerId, taskId)
      consecutiveFailures = 0
      progressDone.value = progressOffset + task.deleted
      if (task.status === 'completed' || task.status === 'failed') {
        return { status: task.status, deleted: task.deleted }
      }
    } catch {
      consecutiveFailures++
      if (consecutiveFailures >= DELETE_POLL_MAX_FAILURES) {
        return { status: 'failed', deleted: 0 }
      }
    }
    await new Promise((resolve) => setTimeout(resolve, DELETE_POLL_INTERVAL_MS))
  }
  return { status: 'failed', deleted: 0 }
}

async function resolveSelectedItems(): Promise<PoolKeySelectionItem[]> {
  if (!props.providerId) return []

  if (selectAllFiltered.value) {
    progressLabel.value = '正在解析筛选结果...'
    const result = await resolvePoolKeySelection(props.providerId, { ...props.selectionFilters })
    return Array.isArray(result.items) ? result.items : []
  }

  const selectedKeys = [...new Map(
    props.selectedKeys
      .filter(key => Boolean(key.key_id))
      .map(key => [key.key_id, key] as const),
  ).values()]
  return selectedKeys.map((key) => {
    return {
      key_id: key.key_id,
      key_name: key.key_name || '',
      auth_type: key.auth_type || 'api_key',
      auth_type_by_format: key.auth_type_by_format,
      allow_auth_channel_mismatch_formats: key.allow_auth_channel_mismatch_formats,
      credential_kind: key.credential_kind,
      runtime_auth_kind: key.runtime_auth_kind,
      oauth_managed: key.oauth_managed,
      oauth_header_auth: key.oauth_header_auth,
      can_refresh_oauth: key.can_refresh_oauth,
      can_export_oauth: key.can_export_oauth,
      can_edit_oauth: key.can_edit_oauth,
    }
  })
}

async function executeAction(actionOverride?: PoolBatchActionValue): Promise<void> {
  if (executing.value) return
  if (actionOverride) {
    selectedAction.value = actionOverride
  }
  if (selectedCount.value === 0) {
    warning('请先选择账号')
    return
  }

  const requestedCount = selectedCount.value
  if (selectedAction.value === 'set_proxy' && !proxyNodeIdForAction.value) {
    warning('请先选择代理节点')
    return
  }
  if (selectedAction.value === 'update_settings' && settingsErrors.value.length > 0) {
    warning(settingsErrors.value[0])
    return
  }

  executing.value = true
  let successCount = 0
  let failedCount = 0
  let skippedCount = 0
  let resolvedCount = 0
  const actionStartedAt = performance.now()
  let actionPhaseMs = 0

  const actionLabel = ACTION_OPTIONS.find((item) => item.value === selectedAction.value)?.label || '执行'
  progressDone.value = 0
  progressTotal.value = 0
  progressLabel.value = selectAllFiltered.value ? '正在解析筛选结果...' : `正在${actionLabel}...`
  lastResultMessage.value = ''

  try {
    const selectedKeys = await resolveSelectedItems()
    resolvedCount = selectedKeys.length
    if (selectedKeys.length === 0) {
      warning('未找到可执行账号，请刷新列表重试')
      return
    }

    progressDone.value = 0
    progressTotal.value = selectedKeys.length
    progressLabel.value = `正在${actionLabel}...`

    if (selectedAction.value === 'refresh_quota') {
      const targetIds = selectedKeys.map((key) => key.key_id)
      const BATCH_SIZE = 20
      const counts = await runChunkedBatchAction({
        items: targetIds,
        chunkSize: BATCH_SIZE,
        runChunk: (batch) => refreshProviderQuota(props.providerId, batch),
        onChunkStart: ({ batchIndex, totalBatches }) => {
          progressLabel.value = `正在${actionLabel}...（第 ${batchIndex}/${totalBatches} 批）`
        },
        onChunkDone: ({ processed }) => {
          progressDone.value = processed
        },
      })
      successCount += counts.success
      failedCount += counts.failed
      skippedCount += counts.skipped
    } else if (selectedAction.value === 'export') {
      const exportableKeys = selectedKeys.filter((key) => canExportOAuthCredential(key))
      const exportedEntries: Array<Record<string, unknown> | null> = Array.from({ length: exportableKeys.length }, () => null)

      skippedCount += selectedKeys.length - exportableKeys.length
      progressDone.value = 0
      progressTotal.value = exportableKeys.length
      if (skippedCount > 0) {
        progressLabel.value = `正在${actionLabel}...（跳过 ${skippedCount} 个非 OAuth 账号）`
      }

      let cursor = 0
      const CONCURRENCY = props.batchConcurrency || 8
      const runNext = async (): Promise<void> => {
        while (cursor < exportableKeys.length) {
          const idx = cursor++
          const key = exportableKeys[idx]
          try {
            exportedEntries[idx] = await exportKey(key.key_id)
            successCount += 1
          } catch (err) {
            failedCount += 1
            // eslint-disable-next-line no-console
            console.error(`[PoolAccountBatchDialog] export failed (${key.key_id}):`, err)
          } finally {
            progressDone.value += 1
          }
        }
      }

      const workers = Array.from(
        { length: Math.min(CONCURRENCY, exportableKeys.length) },
        () => runNext(),
      )
      await Promise.all(workers)

      const exportedData = exportedEntries.filter((item): item is Record<string, unknown> => item !== null)
      if (exportedData.length > 0) {
        downloadJsonFile(exportedData, getBatchExportFilename())
      }
    } else if (selectedAction.value === 'delete') {
      const targetIds = selectedKeys.map((key) => key.key_id)
      const BATCH_SIZE = 2000
      const totalBatches = Math.ceil(targetIds.length / BATCH_SIZE)

      for (let i = 0; i < targetIds.length; i += BATCH_SIZE) {
        const batchIndex = Math.floor(i / BATCH_SIZE) + 1
        const batch = targetIds.slice(i, i + BATCH_SIZE)
        if (totalBatches > 1) {
          progressLabel.value = `正在${actionLabel}...（第 ${batchIndex}/${totalBatches} 批）`
        }

        try {
          const result = await batchActionPoolKeys(props.providerId, {
            key_ids: batch,
            action: 'delete',
          })

          if (result.task_id) {
            progressLabel.value = `正在${actionLabel}...（后台执行中）`
            const taskResult = await pollDeleteTask(props.providerId, result.task_id, i)
            successCount += taskResult.deleted
            if (taskResult.status === 'failed') {
              failedCount += batch.length - taskResult.deleted
            }
          } else {
            successCount += result.affected
          }
        } catch (err) {
          // eslint-disable-next-line no-console
          console.error(`batch delete failed (batch ${batchIndex}/${totalBatches}):`, err)
          failedCount += batch.length
        }

        progressDone.value = Math.min(i + BATCH_SIZE, targetIds.length)
      }
    } else if (['enable', 'disable', 'clear_proxy', 'set_proxy', 'update_settings'].includes(selectedAction.value)) {
      const targetIds = selectedKeys.map((key) => key.key_id)
      const BATCH_SIZE = 2000
      const totalBatches = Math.ceil(targetIds.length / BATCH_SIZE)

      for (let i = 0; i < targetIds.length; i += BATCH_SIZE) {
        const batchIndex = Math.floor(i / BATCH_SIZE) + 1
        const batch = targetIds.slice(i, i + BATCH_SIZE)
        if (totalBatches > 1) {
          progressLabel.value = `正在${actionLabel}...（第 ${batchIndex}/${totalBatches} 批）`
        }

        const payload = selectedAction.value === 'set_proxy'
          ? { node_id: proxyNodeIdForAction.value, enabled: true }
          : selectedAction.value === 'update_settings'
            ? buildPoolKeySettingsPatch(settingsSelection, settingsDraft)
            : undefined

        try {
          const result = await batchActionPoolKeys(props.providerId, {
            key_ids: batch,
            action: selectedAction.value as 'enable' | 'disable' | 'clear_proxy' | 'set_proxy' | 'update_settings',
            ...(payload ? { payload } : {}),
          })
          successCount += result.affected
        } catch (err) {
          // eslint-disable-next-line no-console
          console.error(`batch ${selectedAction.value} failed (batch ${batchIndex}/${totalBatches}):`, err)
          failedCount += batch.length
        }

        progressDone.value = Math.min(i + BATCH_SIZE, targetIds.length)
      }
    } else {
      const CONCURRENCY = props.batchConcurrency || 8
      const tasks: Array<() => Promise<'success' | 'skip'>> = []
      for (const key of selectedKeys) {
        if (selectedAction.value === 'refresh_oauth' && !canRefreshOAuthCredential(key)) {
          skippedCount += 1
          progressDone.value += 1
          continue
        }
        tasks.push(() => refreshProviderOAuth(key.key_id).then(() => 'success' as const))
      }
      progressTotal.value = selectedKeys.length

      let cursor = 0
      const runNext = async (): Promise<void> => {
        while (cursor < tasks.length) {
          const idx = cursor++
          try {
            await tasks[idx]()
            successCount += 1
          } catch {
            failedCount += 1
          }
          progressDone.value += 1
        }
      }
      const workers = Array.from({ length: Math.min(CONCURRENCY, tasks.length) }, () => runNext())
      await Promise.all(workers)
    }

    lastResultMessage.value = `执行完成：成功 ${successCount}，失败 ${failedCount}，跳过 ${skippedCount}`
    if (failedCount > 0 || (selectedAction.value === 'export' && successCount === 0)) warning(lastResultMessage.value)
    else success(lastResultMessage.value)

    actionPhaseMs = performance.now() - actionStartedAt
    if (selectedAction.value !== 'export') {
      emit('changed')
    }
  } catch (err) {
    showError(parseApiError(err, '批量操作失败'))
  } finally {
    // eslint-disable-next-line no-console
    console.info('[PoolAccountBatchDialog] executeAction timing', {
      providerId: props.providerId,
      action: selectedAction.value,
      requestedCount,
      resolvedCount,
      successCount,
      failedCount,
      skippedCount,
      actionPhaseMs: Math.round(actionPhaseMs),
      totalMs: Math.round(performance.now() - actionStartedAt),
    })
    executing.value = false
    progressTotal.value = 0
    progressDone.value = 0
    progressLabel.value = ''
  }
}

watch(
  () => props.modelValue,
  (open) => {
    if (!open) return
    const initialAction = props.initialAction || null
    lastResultMessage.value = ''
    selectedAction.value = initialAction || 'refresh_quota'
    proxyNodeIdForAction.value = ''
    Object.assign(settingsSelection, createPoolKeyBatchSettingSelection())
    Object.assign(settingsDraft, createPoolKeyBatchSettingsDraft())
    proxyNodesStore.ensureLoaded()
    if (initialAction) {
      void nextTick(() => {
        if (!props.modelValue || props.initialAction !== initialAction) return
        handleActionButtonClick(initialAction)
      })
    }
  },
  { immediate: true },
)
</script>
