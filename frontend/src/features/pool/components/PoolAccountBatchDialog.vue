<template>
  <Dialog
    :model-value="modelValue"
    title="账号批量操作"
    :description="dialogDescription"
    size="3xl"
    persistent
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="space-y-4">
      <div class="space-y-3 rounded-lg border bg-muted/20 px-3 py-2.5">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-foreground">快捷多选</span>
          <Button
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-[11px]"
            :disabled="loading || executing || !hasActiveFilters"
            @click="clearFilters"
          >
            重置筛选
          </Button>
        </div>

        <div class="flex flex-wrap gap-2">
          <Button
            v-for="option in QUICK_SELECT_OPTIONS"
            :key="option.value"
            variant="outline"
            size="sm"
            class="h-8 px-2.5 text-[11px]"
            :class="activeQuickSelectorSet.has(option.value) ? 'border-primary/70 bg-primary/10 text-primary' : ''"
            :disabled="loading || executing"
            @click="toggleQuickSelector(option.value)"
          >
            {{ option.label }}
          </Button>
        </div>

        <div class="space-y-3 rounded-md border bg-background/80 px-3 py-3">
          <div class="flex items-center gap-2">
            <Input
              :model-value="searchText"
              placeholder="搜索账号名 / 套餐 / 额度 / 代理状态"
              class="h-8 flex-1"
              @update:model-value="(v) => searchText = String(v || '')"
            />
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8 shrink-0"
              :disabled="loading || executing"
              @click="loadKeysPage()"
            >
              <RefreshCw
                class="h-3.5 w-3.5"
                :class="loading ? 'animate-spin' : ''"
              />
            </Button>
          </div>

          <div class="flex flex-col gap-2 text-xs lg:flex-row lg:items-center lg:justify-between">
            <div class="text-muted-foreground">
              共 {{ filteredTotal }} 个匹配账号，当前页 {{ pageKeys.length }} 个，已选 {{ selectedCount }} 个
            </div>
            <div class="flex flex-wrap items-center gap-1">
              <div class="mr-1 flex items-center gap-2">
                <Checkbox
                  :checked="isAllFilteredSelected"
                  :indeterminate="isPartiallyFilteredSelected"
                  :disabled="filteredTotal === 0 || loading || executing"
                  @update:checked="toggleSelectFiltered"
                />
                <span class="text-muted-foreground">全选筛选结果</span>
              </div>
              <Button
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-[11px]"
                :disabled="pageKeys.length === 0 || loading || executing || selectAllFiltered"
                @click="toggleSelectCurrentPage"
              >
                {{ isCurrentPageFullySelected ? '取消本页全选' : '本页全选' }}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-[11px]"
                :disabled="!canClearSelection || loading || executing"
                @click="clearSelection"
              >
                清空选择
              </Button>
            </div>
          </div>
        </div>
      </div>

      <div class="grid gap-4 lg:grid-cols-[minmax(0,1fr)_19rem]">
        <div class="min-w-0 space-y-3">
          <div class="max-h-[420px] overflow-y-auto rounded-lg border">
            <div
              v-if="loading"
              class="py-10 text-center text-sm text-muted-foreground"
            >
              正在加载账号列表...
            </div>
            <div
              v-else-if="pageKeys.length === 0"
              class="py-10 text-center text-sm text-muted-foreground"
            >
              无匹配账号
            </div>
            <label
              v-for="key in pageKeys"
              :key="key.key_id"
              class="flex items-center gap-2.5 px-3 py-2 border-b last:border-b-0 cursor-pointer hover:bg-muted/30"
            >
              <Checkbox
                :checked="selectAllFiltered || selectedIdSet.has(key.key_id)"
                :disabled="executing || selectAllFiltered"
                @update:checked="(checked) => toggleOne(key.key_id, checked === true)"
              />
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-1.5">
                  <span class="text-xs font-medium truncate">{{ key.key_name || '未命名' }}</span>
                  <Badge
                    variant="outline"
                    class="text-[10px] px-1 py-0 h-4 shrink-0"
                  >{{ normalizeAuthTypeLabel(key.auth_type) }}</Badge>
                  <Badge
                    v-if="getStatusBadgeLabel(key)"
                    variant="destructive"
                    class="text-[10px] px-1 py-0 h-4 shrink-0"
                    :title="getStatusBadgeTitle(key)"
                  >{{ getStatusBadgeLabel(key) }}</Badge>
                  <Badge
                    v-if="key.oauth_plan_type"
                    variant="outline"
                    class="text-[10px] px-1 py-0 h-4 shrink-0"
                  >{{ key.oauth_plan_type }}</Badge>
                  <Badge
                    v-if="getOAuthOrgBadge(key)"
                    variant="secondary"
                    class="text-[10px] px-1 py-0 h-4 shrink-0"
                    :title="getOAuthOrgBadge(key)?.title"
                  >{{ getOAuthOrgBadge(key)?.label }}</Badge>
                </div>
                <div class="flex items-center gap-1.5 mt-0.5 text-[11px] text-muted-foreground flex-wrap">
                  <span :class="key.is_active ? '' : 'text-destructive'">{{ key.is_active ? '启用' : '禁用' }}</span>
                  <span v-if="key.account_quota">{{ shortenQuota(key.account_quota) }}</span>
                  <span v-if="key.proxy?.node_id">独立代理</span>
                  <span
                    v-if="key.last_used_at"
                    class="ml-auto shrink-0"
                  >{{ formatRelativeTime(key.last_used_at) }}</span>
                </div>
              </div>
            </label>
          </div>

          <div
            v-if="totalPages > 1"
            class="flex items-center justify-between text-xs text-muted-foreground"
          >
            <span>第 {{ currentPage }} / {{ totalPages }} 页</span>
            <div class="flex items-center gap-1">
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                :disabled="currentPage <= 1"
                @click="goToPage(1)"
              >
                <ChevronsLeft class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                :disabled="currentPage <= 1"
                @click="goToPage(currentPage - 1)"
              >
                <ChevronLeft class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                :disabled="currentPage >= totalPages"
                @click="goToPage(currentPage + 1)"
              >
                <ChevronRight class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                :disabled="currentPage >= totalPages"
                @click="goToPage(totalPages)"
              >
                <ChevronsRight class="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>
        </div>

        <div class="space-y-3 lg:sticky lg:top-1 lg:self-start">
          <div class="space-y-2 rounded-lg border bg-background px-3 py-3">
            <div class="text-xs font-medium text-foreground">
              执行动作
            </div>
            <div class="text-[11px] text-muted-foreground">
              代理节点（仅“配置代理”动作生效）
            </div>
            <ProxyNodeSelect
              :model-value="proxyNodeIdForAction"
              trigger-class="h-8"
              @update:model-value="(v: string) => proxyNodeIdForAction = v"
            />
            <div class="grid gap-2 sm:grid-cols-2 lg:grid-cols-1">
              <Button
                v-for="item in ACTION_OPTIONS"
                :key="item.value"
                class="h-8 w-full px-3 text-xs"
                :variant="getActionButtonVariant(item)"
                :disabled="!canExecuteSpecifiedAction(item.value)"
                @click="confirmAndExecuteAction(item.value)"
              >
                {{ item.label }}
              </Button>
            </div>
          </div>
        </div>
      </div>

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
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { Dialog, Button, Input, Checkbox, Badge } from '@/components/ui'
import ProxyNodeSelect from '@/features/providers/components/ProxyNodeSelect.vue'
import { RefreshCw, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import {
  listPoolKeys,
  batchActionPoolKeys,
  getPoolBatchDeleteTask,
  resolvePoolKeySelection,
  type PoolKeyDetail,
  type PoolKeySelectionItem,
} from '@/api/endpoints/pool'
import { exportKey, refreshProviderQuota } from '@/api/endpoints/keys'
import { refreshProviderOAuth } from '@/api/endpoints/provider_oauth'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import { getOAuthOrgBadge } from '@/utils/oauthIdentity'
import {
  getAccountStatusDisplay,
  getAccountStatusTitle,
  getOAuthStatusDisplay,
  getOAuthStatusTitle,
} from '@/utils/providerKeyStatus'

type QuickSelectorValue =
  | 'banned'
  | 'no_5h_limit'
  | 'no_weekly_limit'
  | 'plan_free'
  | 'plan_team'
  | 'oauth_invalid'
  | 'proxy_unset'
  | 'proxy_set'
  | 'disabled'
  | 'enabled'

type BatchActionValue =
  | 'export'
  | 'delete'
  | 'refresh_oauth'
  | 'refresh_quota'
  | 'clear_proxy'
  | 'set_proxy'
  | 'enable'
  | 'disable'

type BatchActionOption = {
  value: BatchActionValue
  label: string
  hint: string
  destructive?: boolean
}

const props = defineProps<{
  modelValue: boolean
  providerId: string
  providerName?: string
  providerType?: string
  batchConcurrency?: number | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  changed: []
}>()

const QUICK_SELECT_OPTIONS: Array<{ value: QuickSelectorValue; label: string }> = [
  { value: 'banned', label: '账号异常' },
  { value: 'oauth_invalid', label: 'Token 异常' },
  { value: 'no_5h_limit', label: '无5H限额' },
  { value: 'no_weekly_limit', label: '无周限额' },
  { value: 'plan_free', label: '全部 Free' },
  { value: 'plan_team', label: '全部 Team' },
  { value: 'proxy_unset', label: '未配置代理' },
  { value: 'proxy_set', label: '已配置独立代理' },
  { value: 'disabled', label: '已禁用' },
  { value: 'enabled', label: '已启用' },
]

const ACTION_OPTIONS: BatchActionOption[] = [
  { value: 'refresh_quota', label: '刷新额度', hint: '调用额度刷新接口，适合核对最新配额状态。' },
  { value: 'refresh_oauth', label: '刷新 OAuth', hint: '仅对 OAuth 账号有效，非 OAuth 账号会自动跳过。' },
  { value: 'set_proxy', label: '配置代理', hint: '为选中账号绑定独立代理节点。' },
  { value: 'clear_proxy', label: '清除代理', hint: '移除账号独立代理，回退到提供商默认代理。' },
  { value: 'enable', label: '启用', hint: '批量启用账号，恢复可调度状态。' },
  { value: 'disable', label: '禁用', hint: '批量禁用账号，保留数据但停止调度。' },
  { value: 'export', label: '导出凭据', hint: '仅导出 OAuth 凭据，其他类型账号将被跳过。' },
  { value: 'delete', label: '删除账号', hint: '永久删除账号数据，执行后不可恢复。', destructive: true },
]

const { success, warning, error: showError } = useToast()
const { confirm } = useConfirm()
const proxyNodesStore = useProxyNodesStore()

const loading = ref(false)
const executing = ref(false)
const pageKeys = ref<PoolKeyDetail[]>([])
const filteredTotal = ref(0)
const selectedKeyIds = ref<string[]>([])
const knownKeysById = ref<Record<string, PoolKeyDetail>>({})
const selectAllFiltered = ref(false)
const searchText = ref('')
const selectedAction = ref<BatchActionValue>('refresh_quota')
const proxyNodeIdForAction = ref('')
const lastResultMessage = ref('')
const progressTotal = ref(0)
const progressDone = ref(0)
const progressLabel = ref('')
const activeQuickSelectors = ref<QuickSelectorValue[]>([])
const currentPage = ref(1)

const PAGE_SIZE = 50
const SEARCH_DEBOUNCE_MS = 250

let loadRequestId = 0
let searchDebounceTimer: number | null = null
let suppressFilterWatch = false

const dialogDescription = computed(() => {
  const name = (props.providerName || '').trim()
  return name ? `${name} - 选择账号并批量执行动作` : '选择账号并批量执行动作'
})

const selectedIdSet = computed(() => new Set(selectedKeyIds.value))
const selectedCount = computed(() => (selectAllFiltered.value ? filteredTotal.value : selectedKeyIds.value.length))
const totalPages = computed(() => Math.max(1, Math.ceil(filteredTotal.value / PAGE_SIZE)))
const isAllFilteredSelected = computed(() => selectAllFiltered.value && filteredTotal.value > 0)
const isPartiallyFilteredSelected = computed(() => !selectAllFiltered.value && selectedKeyIds.value.length > 0)
const hasActiveFilters = computed(() => searchText.value.trim().length > 0 || activeQuickSelectors.value.length > 0)
const selectedOnCurrentPageCount = computed(() => {
  if (selectAllFiltered.value) return pageKeys.value.length
  let count = 0
  for (const key of pageKeys.value) {
    if (selectedIdSet.value.has(key.key_id)) count += 1
  }
  return count
})
const isCurrentPageFullySelected = computed(() => {
  if (selectAllFiltered.value || pageKeys.value.length === 0) return false
  return selectedOnCurrentPageCount.value === pageKeys.value.length
})
const canClearSelection = computed(() => selectAllFiltered.value || selectedKeyIds.value.length > 0)
const activeQuickSelectorSet = computed(() => new Set(activeQuickSelectors.value))

function normalizeText(value: unknown): string {
  return String(value || '').trim().toLowerCase()
}

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

function normalizeAuthTypeLabel(authType: string): string {
  const text = normalizeText(authType)
  if (text === 'oauth') return 'OAuth'
  if (text === 'service_account') return 'Service'
  return 'API Key'
}

function getStatusBadgeLabel(key: PoolKeyDetail): string | null {
  const account = getAccountStatusDisplay(key)
  if (account.blocked && account.label) return account.label

  const oauth = getOAuthStatusDisplay(key, 0)
  if (oauth?.isInvalid) return 'Token 失效'
  if (oauth?.isExpired) return 'Token 过期'
  return null
}

function getStatusBadgeTitle(key: PoolKeyDetail): string {
  const label = getStatusBadgeLabel(key)
  if (!label) return ''

  const accountTitle = getAccountStatusTitle(key)
  if (accountTitle) return accountTitle

  const oauthTitle = getOAuthStatusTitle(key, 0)
  return oauthTitle || label
}

function formatRelativeTime(value: string): string {
  const ts = new Date(value).getTime()
  if (!Number.isFinite(ts)) return '-'
  const diff = Date.now() - ts
  if (diff < 60_000) return '刚刚'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}分钟前`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}小时前`
  return `${Math.floor(diff / 86_400_000)}天前`
}

function shortenQuota(raw: string): string {
  return raw.split('|').map((segment) => {
    let value = segment.trim()
    value = value.replace(/剩余\s*/g, '')
    value = value.replace(/％/g, '%')
    value = value.replace(/[（(]\s*(\d+)\s*天\s*(\d+)\s*小时.*?[）)]/g, ' $1d$2h')
    value = value.replace(/[（(]\s*(\d+)\s*小时\s*(\d+)\s*分钟.*?[）)]/g, ' $1h$2m')
    value = value.replace(/[（(]\s*(\d+)\s*小时.*?[）)]/g, ' $1h')
    value = value.replace(/[（(]\s*(\d+)\s*分钟.*?[）)]/g, ' $1m')
    value = value.replace(/[（(]\s*(\d+)\s*天.*?[）)]/g, ' $1d')
    value = value.replace(/[（(].*?[）)]/g, '')
    return value.trim()
  }).join(' | ')
}

function clearSearchDebounce(): void {
  if (searchDebounceTimer !== null) {
    clearTimeout(searchDebounceTimer)
    searchDebounceTimer = null
  }
}

function rememberPageKeys(keys: PoolKeyDetail[]): void {
  if (keys.length === 0) return
  const next = { ...knownKeysById.value }
  for (const key of keys) {
    next[key.key_id] = key
  }
  knownKeysById.value = next
}

function resetSelection(clearKnown = false): void {
  selectAllFiltered.value = false
  selectedKeyIds.value = []
  if (clearKnown) knownKeysById.value = {}
}

function buildSelectionFilters(): { search?: string; quick_selectors?: string[] } {
  const search = searchText.value.trim()
  const quickSelectors = activeQuickSelectors.value.map((value) => String(value))
  return {
    ...(search ? { search } : {}),
    ...(quickSelectors.length > 0 ? { quick_selectors: quickSelectors } : {}),
  }
}

async function loadKeysPage(): Promise<void> {
  if (!props.providerId) {
    pageKeys.value = []
    filteredTotal.value = 0
    resetSelection(true)
    return
  }

  const requestId = ++loadRequestId
  loading.value = true
  const startedAt = performance.now()
  let ok = false
  try {
    const res = await listPoolKeys(props.providerId, {
      page: currentPage.value,
      page_size: PAGE_SIZE,
      status: 'all',
      search: searchText.value.trim() || undefined,
      quick_selectors: activeQuickSelectors.value,
      search_scope: 'full',
    })
    if (requestId !== loadRequestId) return

    pageKeys.value = Array.isArray(res.keys) ? res.keys : []
    filteredTotal.value = Number(res.total || 0)
    rememberPageKeys(pageKeys.value)
    ok = true
  } catch (err) {
    if (requestId !== loadRequestId) return
    pageKeys.value = []
    filteredTotal.value = 0
    showError(parseApiError(err, '加载账号列表失败'))
  } finally {
    if (requestId === loadRequestId) {
      loading.value = false
      // eslint-disable-next-line no-console
      console.info('[PoolAccountBatchDialog] loadKeysPage timing', {
        providerId: props.providerId,
        page: currentPage.value,
        pageSize: PAGE_SIZE,
        search: searchText.value.trim(),
        quickSelectors: activeQuickSelectors.value,
        total: filteredTotal.value,
        count: pageKeys.value.length,
        ok,
        durationMs: Math.round(performance.now() - startedAt),
      })
    }
  }
}

function requestFilteredReload(debounceMs = 0): void {
  if (!props.modelValue) return
  clearSearchDebounce()
  resetSelection()
  lastResultMessage.value = ''
  const run = () => {
    searchDebounceTimer = null
    currentPage.value = 1
    void loadKeysPage()
  }
  if (debounceMs > 0) {
    searchDebounceTimer = window.setTimeout(run, debounceMs)
  } else {
    run()
  }
}

async function goToPage(page: number): Promise<void> {
  const nextPage = Math.min(Math.max(1, page), totalPages.value)
  currentPage.value = nextPage
  await loadKeysPage()
}

function toggleOne(keyId: string, checked: boolean): void {
  const set = new Set(selectedKeyIds.value)
  if (checked) set.add(keyId)
  else set.delete(keyId)
  selectedKeyIds.value = [...set]
}

function toggleSelectFiltered(checked: boolean | 'indeterminate'): void {
  selectAllFiltered.value = checked === true
  if (selectAllFiltered.value) {
    selectedKeyIds.value = []
  }
}

function toggleSelectCurrentPage(): void {
  if (selectAllFiltered.value || pageKeys.value.length === 0) return
  const set = new Set(selectedKeyIds.value)
  const pageIds = pageKeys.value.map((key) => key.key_id)
  const shouldUnselect = pageIds.every((id) => set.has(id))
  for (const id of pageIds) {
    if (shouldUnselect) set.delete(id)
    else set.add(id)
  }
  selectedKeyIds.value = [...set]
}

function clearSelection(): void {
  resetSelection()
}

function clearFilters(): void {
  if (!hasActiveFilters.value) return
  clearSearchDebounce()
  suppressFilterWatch = true
  searchText.value = ''
  activeQuickSelectors.value = []
  suppressFilterWatch = false
  requestFilteredReload()
}

function toggleQuickSelector(selector: QuickSelectorValue): void {
  const idx = activeQuickSelectors.value.indexOf(selector)
  if (idx >= 0) {
    activeQuickSelectors.value.splice(idx, 1)
  } else {
    activeQuickSelectors.value.push(selector)
  }
  requestFilteredReload()
}

function canExecuteSpecifiedAction(action: BatchActionValue): boolean {
  if (executing.value || loading.value || selectedCount.value === 0) return false
  if (action === 'set_proxy') return Boolean(proxyNodeIdForAction.value)
  return true
}

function getActionButtonVariant(option: BatchActionOption): 'default' | 'destructive' | 'outline' {
  if (option.destructive) return 'destructive'
  return 'outline'
}

async function confirmAndExecuteAction(action: BatchActionValue): Promise<void> {
  selectedAction.value = action
  if (selectedCount.value === 0) {
    warning('请先选择账号')
    return
  }
  if (action === 'set_proxy' && !proxyNodeIdForAction.value) {
    warning('请先选择代理节点')
    return
  }
  if (!canExecuteSpecifiedAction(action)) return

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
    const result = await resolvePoolKeySelection(props.providerId, buildSelectionFilters())
    return Array.isArray(result.items) ? result.items : []
  }

  return selectedKeyIds.value.map((keyId) => {
    const key = knownKeysById.value[keyId]
    return {
      key_id: keyId,
      key_name: key?.key_name || '',
      auth_type: key?.auth_type || 'api_key',
    }
  })
}

async function executeAction(actionOverride?: BatchActionValue): Promise<void> {
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

  executing.value = true
  let successCount = 0
  let failedCount = 0
  let skippedCount = 0
  let resolvedCount = 0
  const actionStartedAt = performance.now()
  let actionPhaseMs = 0
  let reloadPhaseMs = 0

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
      const totalBatches = Math.ceil(targetIds.length / BATCH_SIZE)

      for (let i = 0; i < targetIds.length; i += BATCH_SIZE) {
        const batchIndex = Math.floor(i / BATCH_SIZE) + 1
        const batch = targetIds.slice(i, i + BATCH_SIZE)
        progressLabel.value = `正在${actionLabel}...（第 ${batchIndex}/${totalBatches} 批）`

        try {
          const result = await refreshProviderQuota(props.providerId, batch)
          successCount += Number(result.success || 0)
          failedCount += Number(result.failed || 0)
          skippedCount += Math.max(0, batch.length - Number(result.total || 0))
        } catch {
          failedCount += batch.length
        }

        progressDone.value = Math.min(i + BATCH_SIZE, targetIds.length)
      }
    } else if (selectedAction.value === 'export') {
      const exportableKeys = selectedKeys.filter((key) => normalizeText(key.auth_type) === 'oauth')
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
    } else if (['enable', 'disable', 'clear_proxy', 'set_proxy'].includes(selectedAction.value)) {
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
          : undefined

        try {
          const result = await batchActionPoolKeys(props.providerId, {
            key_ids: batch,
            action: selectedAction.value as 'enable' | 'disable' | 'clear_proxy' | 'set_proxy',
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
        if (selectedAction.value === 'refresh_oauth' && normalizeText(key.auth_type) !== 'oauth') {
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
      const reloadStartedAt = performance.now()
      if (selectedAction.value === 'delete' && successCount > 0) {
        resetSelection(true)
      }
      await loadKeysPage()
      if (pageKeys.value.length === 0 && filteredTotal.value > 0 && currentPage.value > totalPages.value) {
        await goToPage(totalPages.value)
      }
      reloadPhaseMs = performance.now() - reloadStartedAt
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
      reloadPhaseMs: Math.round(reloadPhaseMs),
      totalMs: Math.round(performance.now() - actionStartedAt),
    })
    executing.value = false
    progressTotal.value = 0
    progressDone.value = 0
    progressLabel.value = ''
  }
}

watch(searchText, () => {
  if (suppressFilterWatch || !props.modelValue) return
  requestFilteredReload(SEARCH_DEBOUNCE_MS)
})

watch(
  () => props.modelValue,
  (open) => {
    if (!open) {
      clearSearchDebounce()
      return
    }
    suppressFilterWatch = true
    searchText.value = ''
    lastResultMessage.value = ''
    activeQuickSelectors.value = []
    selectedAction.value = 'refresh_quota'
    proxyNodeIdForAction.value = ''
    resetSelection(true)
    filteredTotal.value = 0
    pageKeys.value = []
    currentPage.value = 1
    suppressFilterWatch = false
    proxyNodesStore.ensureLoaded()
    void loadKeysPage()
  },
)

watch(
  () => props.providerId,
  (newId, oldId) => {
    if (!props.modelValue || !newId || newId === oldId) return
    clearSearchDebounce()
    suppressFilterWatch = true
    resetSelection(true)
    filteredTotal.value = 0
    pageKeys.value = []
    currentPage.value = 1
    suppressFilterWatch = false
    void loadKeysPage()
  },
)

onBeforeUnmount(() => {
  clearSearchDebounce()
})
</script>
