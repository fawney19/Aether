<template>
  <Dialog
    :model-value="open"
    :title="legacyT('用户批量操作')"
    :description="legacyT('按当前选择批量调整用户状态、角色和额度')"
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
} from '@/components/ui'
import { useUsersStore } from '@/stores/users'
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
import type {
  UserBatchAccessControlPayload,
  UserBatchAction,
  UserBatchActionRequest,
  UserBatchActionResponse,
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
const { success, warning, error } = useToast()
const { legacyT, locale } = useI18n()

const selectedAction = ref<UserBatchAction>('enable')
const targetRole = ref<UserRole>('user')
const quotaMode = ref<UserBatchQuotaMode>('skip')
const selectedGroupIds = ref<string[]>([])
const previewLoading = ref(false)
const previewItems = ref<UserBatchSelectionItem[]>([])
const resolvedTotal = ref<number | null>(null)
const executing = ref(false)
const lastResult = ref<UserBatchActionResponse | null>(null)

const hasAnyTarget = computed(() => props.selectedCount > 0 || selectedGroupIds.value.length > 0)
const impactCount = computed(() => resolvedTotal.value ?? props.selectedCount)
const canExecute = computed(() => hasAnyTarget.value && !previewLoading.value && !executing.value)
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
const lastResultLabel = computed(() => {
  if (!lastResult.value) return ''
  return legacyT(`成功 ${lastResult.value.success} 个，失败 ${lastResult.value.failed} 个`)
})
const lastResultFailuresLabel = computed(() => {
  if (!lastResult.value || lastResult.value.failures.length === 0) return ''
  const failures = lastResult.value.failures.slice(0, 3).map((item) => `${item.user_id} ${item.reason}`).join(locale.value === 'en-US' ? '; ' : '；')
  return locale.value === 'en-US' ? `: ${failures}` : `：${failures}`
})

watch(
  () => props.open,
  (open) => {
    if (!open) return
    resetLocalState()
    void resolvePreview()
  },
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
  selectedGroupIds.value = []
  lastResult.value = null
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
  let request: UserBatchActionRequest
  if (selectedAction.value === 'update_access_control') {
    const payload = buildAccessControlPayload()
    if (payload === null) {
      warning(legacyT('请选择要修改的额度'))
      return
    }
    request = { selection, action: 'update_access_control', payload }
  } else if (selectedAction.value === 'update_role') {
    request = { selection, action: 'update_role', payload: buildRolePayload() }
  } else {
    request = { selection, action: selectedAction.value }
  }

  executing.value = true
  try {
    const result = await usersStore.batchAction(request)
    lastResult.value = result
    const message = legacyT(`批量操作完成：成功 ${result.success} 个，失败 ${result.failed} 个`)
    if (result.failed > 0) {
      warning(message)
    } else {
      success(message)
    }
    emit('completed', result)
  } catch (err) {
    error(parseApiError(err, '批量操作失败'), legacyT('批量操作失败'))
  } finally {
    executing.value = false
  }
}
</script>
