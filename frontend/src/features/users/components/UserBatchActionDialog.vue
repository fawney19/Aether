<template>
  <Dialog
    :model-value="open"
    :title="legacyT('用户批量操作')"
    :description="legacyT('按当前选择批量调整用户状态、角色、额度和钱包余额')"
    size="2xl"
    persistent
    @update:model-value="handleDialogUpdate"
  >
    <div class="space-y-5">
      <UserBatchTargetSummary
        :select-all-filtered="selectAllFiltered"
        :impact-label="impactLabel"
        :impact-count="impactCount"
        :overflow-preview-label="overflowPreviewLabel"
        :loading="previewLoading"
        :preview-items="previewItems"
      />

      <div class="space-y-2.5">
        <UserBatchGroupPicker
          v-model="selectedGroupIds"
          :groups="groups"
        />
        <UserBatchActionCards
          v-model="selectedAction"
          :actions="USER_BATCH_ACTION_OPTIONS"
        />
      </div>

      <UserBatchRolePanel
        v-if="selectedAction === 'update_role'"
        v-model="targetRole"
        :warning-text="targetRoleWarning"
      />

      <UserBatchQuotaPanel
        v-if="selectedAction === 'update_access_control'"
        v-model="quotaMode"
      />

      <div
        v-if="selectedAction === 'adjust_wallet_balance'"
        class="space-y-3 rounded-xl border border-border bg-background p-4"
      >
        <div class="flex flex-wrap items-center justify-between gap-3">
          <Label for="user-batch-wallet-amount" class="text-sm font-medium">
            {{ legacyT('调整金额 (USD)') }}
          </Label>
          <div class="inline-flex rounded-md border border-border p-0.5" role="group" :aria-label="legacyT('余额调整方式')">
            <Button
              type="button"
              size="sm"
              :variant="balanceOperation === 'add' ? 'default' : 'ghost'"
              :aria-pressed="balanceOperation === 'add'"
              @click="balanceOperation = 'add'"
            >
              <Plus class="mr-1.5 h-4 w-4" />
              {{ legacyT('增加') }}
            </Button>
            <Button
              type="button"
              size="sm"
              :variant="balanceOperation === 'deduct' ? 'default' : 'ghost'"
              :aria-pressed="balanceOperation === 'deduct'"
              @click="balanceOperation = 'deduct'"
            >
              <Minus class="mr-1.5 h-4 w-4" />
              {{ legacyT('扣减') }}
            </Button>
          </div>
        </div>
        <Input
          id="user-batch-wallet-amount"
          :model-value="balanceAmount"
          type="number"
          min="0"
          step="any"
          inputmode="decimal"
          :aria-invalid="balanceAmount !== '' && balancePayload === null"
          @update:model-value="balanceAmount = String($event)"
        />
        <p v-if="balanceAmount !== '' && balancePayload === null" class="text-xs text-destructive">
          {{ legacyT('请输入大于 0 的有限金额') }}
        </p>
        <p class="text-xs leading-relaxed text-muted-foreground">
          {{ legacyT('扣减超过单个用户可用余额时，该用户余额将归零。') }}
        </p>
      </div>

      <div
        v-if="pendingWalletBatch"
        role="alert"
        class="space-y-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-3 text-sm text-amber-900 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-100"
      >
        <p>
          {{ legacyT('存在未决的钱包批量调整') }}：{{ pendingWalletOperationLabel }} {{ pendingWalletBatch.request.payload.amount }} USD。{{ legacyT('结果未知或可能部分完成。请重试原请求，不要开始新的余额调整。') }}
        </p>
        <p
          v-if="walletRequestMismatch"
          class="text-xs"
        >
          {{ legacyT('当前表单与原请求不同；新钱包调整已禁用，请先重试原请求。') }}
        </p>
        <Button
          type="button"
          size="sm"
          variant="outline"
          :disabled="executing"
          @click="retryPendingWalletBatch"
        >
          <RotateCcw class="mr-1.5 h-4 w-4" />
          {{ legacyT('重试原钱包批次') }}
        </Button>
      </div>
      <p
        v-else-if="pendingWalletReadError"
        role="alert"
        class="rounded-md border border-amber-300 bg-amber-50 px-3 py-2.5 text-sm text-amber-900 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-100"
      >
        {{ legacyT('无法读取未决的钱包批量请求。为避免重复扣款，钱包余额调整已禁用；请先核对余额操作结果。') }}
      </p>

      <UserBatchResultSummary
        :result="lastResult"
        :label="lastResultLabel"
        :failures-label="lastResultFailuresLabel"
      />
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="executing"
        @click="emit('close')"
      >
        {{ legacyT('关闭') }}
      </Button>
      <Button
        :disabled="!canExecute"
        @click="executeBatchAction"
      >
        {{ executing ? legacyT('执行中...') : executeButtonLabel }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Dialog,
  Button,
  Input,
  Label,
} from '@/components/ui'
import { Minus, Plus, RotateCcw } from 'lucide-vue-next'
import { useUsersStore } from '@/stores/users'
import { useAuthStore } from '@/stores/auth'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { useI18n } from '@/i18n'
import UserBatchActionCards from './UserBatchActionCards.vue'
import UserBatchGroupPicker from './UserBatchGroupPicker.vue'
import UserBatchQuotaPanel from './UserBatchQuotaPanel.vue'
import UserBatchResultSummary from './UserBatchResultSummary.vue'
import UserBatchRolePanel from './UserBatchRolePanel.vue'
import UserBatchTargetSummary from './UserBatchTargetSummary.vue'
import { USER_BATCH_ACTION_OPTIONS } from './user-management-config'
import type { UserBatchQuotaMode } from './user-management-types'
import { buildUserBatchBalanceAdjustmentPayload } from '@/api/users'
import {
  createUserBatchWalletRetryCoordinator,
  matchesPendingWalletRequest,
  WalletIdempotencyPersistenceUnavailableError,
  WalletIdempotencyScopeChangedError,
  WalletIdempotencyScopeUnavailableError,
  WalletIdempotencyUnavailableError,
} from '../utils/userBatchWalletIdempotency'
import type {
  PendingUserBatchWalletRequest,
  UserBatchWalletAdjustmentRequest,
} from '../utils/userBatchWalletIdempotency'
import type {
  UserBatchAccessControlPayload,
  UserBatchAction,
  UserBatchActionRequest,
  UserBatchActionResponse,
  UserBatchBalanceOperation,
  UserBatchRolePayload,
  UserBatchSelection,
  UserBatchSelectionFilters,
  UserBatchSelectionItem,
  UserRole,
  UserGroup,
} from '@/api/users'

const props = defineProps<{
  open: boolean
  selectedIds: string[]
  selectAllFiltered: boolean
  selectedCount: number
  filters: UserBatchSelectionFilters
  groups: UserGroup[]
}>()

const emit = defineEmits<{
  close: []
  completed: [result: UserBatchActionResponse]
}>()

const usersStore = useUsersStore()
const authStore = useAuthStore()
const walletRetryCoordinator = createUserBatchWalletRetryCoordinator({
  scope: () => authStore.user?.id ?? null,
})
const { success, warning, error } = useToast()
const { legacyT, locale } = useI18n()

const selectedAction = ref<UserBatchAction>('enable')
const targetRole = ref<UserRole>('user')
const quotaMode = ref<UserBatchQuotaMode>('skip')
const balanceOperation = ref<UserBatchBalanceOperation>('add')
const balanceAmount = ref('')
const selectedGroupIds = ref<string[]>([])
const previewLoading = ref(false)
const previewItems = ref<UserBatchSelectionItem[]>([])
const resolvedTotal = ref<number | null>(null)
const executing = ref(false)
const lastResult = ref<UserBatchActionResponse | null>(null)
const pendingWalletBatch = ref<PendingUserBatchWalletRequest | null>(null)
const pendingWalletReadError = ref(false)

const hasAnyTarget = computed(() => props.selectedCount > 0 || selectedGroupIds.value.length > 0)
const impactCount = computed(() => resolvedTotal.value ?? props.selectedCount)
const balancePayload = computed(() => buildUserBatchBalanceAdjustmentPayload(
  balanceOperation.value,
  balanceAmount.value,
))
const walletAdjustmentRequest = computed<UserBatchWalletAdjustmentRequest | null>(() => (
  balancePayload.value === null
    ? null
    : { selection: buildSelection(), action: 'adjust_wallet_balance', payload: balancePayload.value }
))
const walletRequestMismatch = computed(() => (
  pendingWalletBatch.value !== null
  && selectedAction.value === 'adjust_wallet_balance'
  && (walletAdjustmentRequest.value === null
    || !matchesPendingWalletRequest(pendingWalletBatch.value, walletAdjustmentRequest.value))
))
const canExecute = computed(() => (
  hasAnyTarget.value
  && !previewLoading.value
  && !executing.value
  && (selectedAction.value !== 'adjust_wallet_balance'
    || (balancePayload.value !== null && !pendingWalletReadError.value && !walletRequestMismatch.value))
))
const selectedActionLabel = computed(() => (
  USER_BATCH_ACTION_OPTIONS.find((action) => action.value === selectedAction.value)?.label ?? '批量操作'
))
const impactLabel = computed(() => locale.value === 'en-US'
  ? `Affected users: ${impactCount.value}`
  : `影响用户：${impactCount.value} 个`)
const overflowPreviewLabel = computed(() => legacyT(`等 ${impactCount.value} 个用户`))
const targetRoleWarning = computed(() => {
  if (targetRole.value === 'admin') {
    return legacyT('提示：设置为管理员会授予用户完整后台管理能力。')
  }
  if (targetRole.value === 'audit_admin') {
    return legacyT('提示：设置为审计管理员会授予后台只读查看能力。')
  }
  return legacyT('提示：设置为普通用户会移除目标用户的管理员权限。')
})
const executeButtonLabel = computed(() => legacyT(`确认${selectedActionLabel.value}（${impactCount.value}）`))
const pendingWalletOperationLabel = computed(() => (
  pendingWalletBatch.value?.request.payload.operation === 'deduct'
    ? legacyT('扣减')
    : legacyT('增加')
))
const lastResultLabel = computed(() => {
  if (!lastResult.value) return ''
  if (lastResult.value.interrupted) {
    return legacyT(
      `批量操作中断：成功 ${lastResult.value.success} 个，结果待确认 ${lastResult.value.uncertain_user_ids?.length ?? 0} 个，尚未执行 ${lastResult.value.unprocessed_user_ids?.length ?? 0} 个`,
    )
  }
  return legacyT(`成功 ${lastResult.value.success} 个，失败 ${lastResult.value.failed} 个`)
})
const lastResultFailuresLabel = computed(() => {
  if (!lastResult.value || lastResult.value.failures.length === 0) return ''
  const failures = lastResult.value.failures.slice(0, 3)
    .map((item) => `${item.user_id} ${legacyT(item.reason)}`)
    .join(locale.value === 'en-US' ? '; ' : '；')
  return locale.value === 'en-US' ? `: ${failures}` : `：${failures}`
})

watch(
  () => props.open,
  (open) => {
    if (!open) return
    resetLocalState()
    refreshPendingWalletBatch()
    void resolvePreview()
  },
  { immediate: true },
)

watch(
  () => authStore.user?.id,
  () => refreshPendingWalletBatch(),
)

watch(
  () => [props.selectedIds, props.selectAllFiltered, props.selectedCount, props.filters] as const,
  () => {
    if (props.open) void resolvePreview()
  },
)

function handleDialogUpdate(value: boolean): void {
  if (!value) emit('close')
}

function resetLocalState(): void {
  selectedAction.value = 'enable'
  targetRole.value = 'user'
  quotaMode.value = 'skip'
  balanceOperation.value = 'add'
  balanceAmount.value = ''
  selectedGroupIds.value = []
  lastResult.value = null
}

function refreshPendingWalletBatch(): void {
  try {
    pendingWalletBatch.value = walletRetryCoordinator.getPending()
    pendingWalletReadError.value = false
  } catch {
    pendingWalletBatch.value = null
    pendingWalletReadError.value = true
  }
}

function buildSelection(): UserBatchSelection {
  const group_ids = selectedGroupIds.value.length > 0 ? [...selectedGroupIds.value] : undefined
  if (props.selectAllFiltered) {
    return { filters: props.filters, group_ids }
  }
  return { user_ids: [...props.selectedIds], group_ids }
}

async function resolvePreview(): Promise<void> {
  if (!hasAnyTarget.value) {
    resolvedTotal.value = 0
    previewItems.value = []
    return
  }
  previewLoading.value = true
  try {
    const result = await usersStore.resolveBatchSelection(buildSelection())
    resolvedTotal.value = result.total
    previewItems.value = result.items.slice(0, 6)
  } catch (err) {
    resolvedTotal.value = props.selectedCount
    previewItems.value = []
    error(parseApiError(err, '解析用户选择失败'), legacyT('解析用户选择失败'))
  } finally {
    previewLoading.value = false
  }
}

watch(selectedGroupIds, () => {
  if (props.open) void resolvePreview()
})

function buildAccessControlPayload(): UserBatchAccessControlPayload | null {
  const payload: UserBatchAccessControlPayload = {}
  if (quotaMode.value === 'wallet') payload.unlimited = false
  if (quotaMode.value === 'unlimited') payload.unlimited = true
  return Object.keys(payload).length > 0 ? payload : null
}

function buildRolePayload(): UserBatchRolePayload {
  return { role: targetRole.value }
}

async function executeBatchAction(): Promise<void> {
  if (!canExecute.value) return
  const selection = buildSelection()
  let request: Exclude<UserBatchActionRequest, { action: 'adjust_wallet_balance' }> | null = null
  let walletRequest: UserBatchWalletAdjustmentRequest | null = null
  if (selectedAction.value === 'update_access_control') {
    const payload = buildAccessControlPayload()
    if (payload === null) {
      warning(legacyT('请选择要修改的额度'))
      return
    }
    request = { selection, action: 'update_access_control', payload }
  } else if (selectedAction.value === 'adjust_wallet_balance') {
    if (walletAdjustmentRequest.value === null) {
      warning(legacyT('请输入大于 0 的有限金额'))
      return
    }
    walletRequest = walletAdjustmentRequest.value
  } else if (selectedAction.value === 'update_role') {
    request = { selection, action: 'update_role', payload: buildRolePayload() }
  } else {
    request = { selection, action: selectedAction.value }
  }

  executing.value = true
  try {
    if (walletRequest) {
      const result = await walletRetryCoordinator.execute(
        walletRequest,
        (keyedRequest) => usersStore.batchAction(keyedRequest),
      )
      handleWalletBatchResult(result)
      return
    }
    if (request === null) return
    const result = await usersStore.batchAction(request)
    lastResult.value = result
    if (result.interrupted) {
      warning(`${lastResultLabel.value}；${legacyT('请核对余额后再重试，勿直接重试整批')}`)
      emit('completed', result)
      return
    }
    const message = legacyT(`批量操作完成：成功 ${result.success} 个，失败 ${result.failed} 个`)
    if (result.failed > 0) {
      warning(message)
    } else {
      success(message)
    }
    emit('completed', result)
  } catch (err) {
    if (walletRequest) {
      refreshPendingWalletBatch()
      if (err instanceof WalletIdempotencyPersistenceUnavailableError) {
        warning(legacyT('浏览器无法安全保存钱包批量请求，本次请求未发送。'))
      } else if (
        err instanceof WalletIdempotencyUnavailableError
        || err instanceof WalletIdempotencyScopeUnavailableError
        || err instanceof WalletIdempotencyScopeChangedError
      ) {
        warning(legacyT('无法确认管理员身份或安全生成钱包批量请求标识，请求未发送。'))
      } else {
        warning(legacyT('钱包批量调整结果未知或可能部分完成。请重试原请求，不要开始新的余额调整。'))
      }
    } else {
      error(legacyT(parseApiError(err, '批量操作失败')), legacyT('批量操作失败'))
    }
  } finally {
    executing.value = false
  }
}

async function retryPendingWalletBatch(): Promise<void> {
  if (executing.value) return
  executing.value = true
  try {
    const result = await walletRetryCoordinator.retry(
      (request) => usersStore.batchAction(request),
    )
    if (result) handleWalletBatchResult(result)
    else refreshPendingWalletBatch()
  } catch (err) {
    refreshPendingWalletBatch()
    if (err instanceof WalletIdempotencyPersistenceUnavailableError) {
      warning(legacyT('浏览器无法安全保存钱包批量请求，本次请求未发送。'))
    } else if (
      err instanceof WalletIdempotencyScopeUnavailableError
      || err instanceof WalletIdempotencyScopeChangedError
    ) {
      warning(legacyT('无法确认管理员身份，请求未发送。'))
    } else {
      warning(legacyT('钱包批量调整结果未知或可能部分完成。请重试原请求，不要开始新的余额调整。'))
    }
  } finally {
    executing.value = false
  }
}

function handleWalletBatchResult(result: UserBatchActionResponse): void {
  lastResult.value = result
  refreshPendingWalletBatch()
  if (result.interrupted) {
    warning(`${lastResultLabel.value}；${legacyT('结果可能部分完成，请仅重试原请求，不要开始新的余额调整。')}`)
  } else {
    const message = legacyT(`批量操作完成：成功 ${result.success} 个，失败 ${result.failed} 个`)
    if (result.failed > 0) warning(message)
    else success(message)
  }
  emit('completed', result)
}
</script>
