<template>
  <Dialog
    :model-value="open"
    :title="legacyT('用户分组')"
    :description="legacyT('管理用户组、默认注册组、成员和组级访问控制')"
    size="4xl"
    persistent
    @update:model-value="handleDialogUpdate"
  >
    <div class="grid gap-3 lg:min-h-[560px] lg:grid-cols-[17rem_minmax(0,1fr)] lg:gap-4">
      <UserGroupListPanel
        :loading="loading"
        :groups="groups"
        :selected-group-id="editingGroupId"
        @create="startCreate"
        @select="selectGroup"
      />

      <div class="min-w-0 bg-background sm:rounded-xl sm:border sm:border-border/70 sm:p-4">
        <UserGroupEditorHeader
          :editing="Boolean(editingGroupId)"
          :is-default="Boolean(selectedGroup?.is_default)"
          :saving="saving"
          @set-default="toggleDefault"
          @delete="deleteSelectedGroup"
        />

        <div class="space-y-5">
          <UserGroupProfileFields
            :name="form.name"
            :member-user-ids="memberUserIds"
            :user-options="userOptions"
            :members-disabled="Boolean(selectedGroup?.is_default)"
            @update:name="form.name = $event"
            @update:member-user-ids="memberUserIds = $event"
          />

          <UserGroupAccessControlFields
            v-model:form="form"
            :provider-options="providerOptions"
            :api-format-options="apiFormatOptions"
            :model-options="modelOptions"
            :help-text="groupPolicyHelpTextLocalized"
          />
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="saving"
        @click="emit('close')"
      >
        {{ legacyT('关闭') }}
      </Button>
      <Button
        :disabled="saving || !form.name.trim()"
        @click="saveGroup"
      >
        {{ legacyT('保存') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Button,
  Dialog,
} from '@/components/ui'
import { useUsersStore } from '@/stores/users'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import { useI18n } from '@/i18n'
import { useUserAccessControlOptions } from '@/features/users/composables/useUserAccessControlOptions'
import UserGroupAccessControlFields from './UserGroupAccessControlFields.vue'
import UserGroupEditorHeader from './UserGroupEditorHeader.vue'
import UserGroupListPanel from './UserGroupListPanel.vue'
import UserGroupProfileFields from './UserGroupProfileFields.vue'
import type {
  ListPolicyMode,
  RateLimitPolicyMode,
  UpsertUserGroupRequest,
  User,
  UserGroup,
} from '@/api/users'
import type { UserGroupFormState } from './user-management-types'

const props = defineProps<{
  open: boolean
  usersVersion: number
}>()

const emit = defineEmits<{
  close: []
  changed: []
}>()

const usersStore = useUsersStore()
const { success, error } = useToast()
const { confirmDanger, confirmInfo } = useConfirm()
const { legacyT, locale } = useI18n()
const {
  providerOptions,
  apiFormatOptions,
  modelOptions,
  loadAccessControlOptions,
} = useUserAccessControlOptions()

const loading = ref(false)
const saving = ref(false)
const groups = ref<UserGroup[]>([])
const dialogUsers = ref<User[]>([])
const editingGroupId = ref<string | null>(null)
const memberUserIds = ref<string[]>([])
const USER_OPTIONS_CACHE_TTL_MS = 30 * 1000
let dialogUsersLoadedAt = 0
let dialogUsersLoadedVersion = -1

const groupPolicyHelpText = '模型、供应商和端点会在多个用户组之间叠加授权；unrestricted 仍表示不限制，deny_all 只是不授予额外权限。速率限制按付费档位取更高额度，0 表示不限速；用户/API Key 自身限制仍会收窄最终权限。'
const groupPolicyHelpTextLocalized = computed(() => locale.value === 'en-US'
  ? 'Models, providers, and endpoints accumulate across multiple user groups. unrestricted still means no restriction, while deny_all grants no extra permission. Rate limits take the higher quota by tier, and 0 means unlimited. User/API key limits still narrow the final permissions.'
  : groupPolicyHelpText)

const form = ref<UserGroupFormState>(createEmptyForm())

const selectedGroup = computed(() => groups.value.find((group) => group.id === editingGroupId.value) ?? null)
const userOptions = computed(() => dialogUsers.value.map((user) => ({
  label: `${user.username}${user.email ? ` (${user.email})` : ''}`,
  value: user.id,
})))

watch(
  () => props.open,
  (open) => {
    if (!open) return
    void loadDialogData()
    void loadAccessControlOptions().catch((err) => {
      error(parseApiError(err, '加载访问控制选项失败'), legacyT('加载访问控制选项失败'))
    })
  },
)

function handleDialogUpdate(value: boolean): void {
  if (!value) emit('close')
}

async function loadDialogData(): Promise<void> {
  loading.value = true
  try {
    const [groupsResponse] = await Promise.all([
      usersStore.listUserGroups(),
      ensureDialogUsers(),
    ])
    groups.value = groupsResponse.items
    if (editingGroupId.value && !groups.value.some((group) => group.id === editingGroupId.value)) {
      editingGroupId.value = null
    }
    const nextGroup = editingGroupId.value
      ? groups.value.find((group) => group.id === editingGroupId.value) ?? null
      : groups.value[0] ?? null
    if (nextGroup) {
      await selectGroup(nextGroup.id)
    } else {
      startCreate()
    }
  } catch (err) {
    error(parseApiError(err, '加载用户分组失败'), legacyT('加载用户分组失败'))
  } finally {
    loading.value = false
  }
}

async function ensureDialogUsers(): Promise<void> {
  const now = Date.now()
  const isSameUserVersion = dialogUsersLoadedVersion === props.usersVersion
  if (
    isSameUserVersion
    && dialogUsersLoadedAt > 0
    && now - dialogUsersLoadedAt < USER_OPTIONS_CACHE_TTL_MS
  ) {
    return
  }

  dialogUsers.value = await usersStore.listAllUsers({
    cacheTtlMs: isSameUserVersion ? USER_OPTIONS_CACHE_TTL_MS : 0,
    cacheKeySuffix: isSameUserVersion ? undefined : `users-version-${props.usersVersion}`,
  })
  dialogUsersLoadedAt = Date.now()
  dialogUsersLoadedVersion = props.usersVersion
}

async function selectGroup(groupId: string): Promise<void> {
  const group = groups.value.find((item) => item.id === groupId)
  if (!group) return
  editingGroupId.value = group.id
  form.value = {
    name: group.name,
    allowed_providers_mode: normalizeListMode(group.allowed_providers_mode),
    allowed_api_formats_mode: normalizeListMode(group.allowed_api_formats_mode),
    allowed_models_mode: normalizeListMode(group.allowed_models_mode),
    allowed_providers: group.allowed_providers ? [...group.allowed_providers] : [],
    allowed_api_formats: group.allowed_api_formats ? [...group.allowed_api_formats] : [],
    allowed_models: group.allowed_models ? [...group.allowed_models] : [],
    rate_limit_mode: normalizeRateMode(group.rate_limit_mode),
    rate_limit: group.rate_limit ?? undefined,
  }
  try {
    const members = await usersStore.listUserGroupMembers(group.id)
    memberUserIds.value = members.map((member) => member.user_id)
  } catch (err) {
    memberUserIds.value = []
    error(parseApiError(err, '加载分组成员失败'), legacyT('加载分组成员失败'))
  }
}

function normalizeListMode(mode: ListPolicyMode): ListPolicyMode {
  return mode === 'specific' ? 'specific' : 'unrestricted'
}

function normalizeRateMode(mode: RateLimitPolicyMode): RateLimitPolicyMode {
  return mode === 'custom' ? 'custom' : 'system'
}

function createEmptyForm(): UserGroupFormState {
  return {
    name: '',
    allowed_providers_mode: 'unrestricted',
    allowed_api_formats_mode: 'unrestricted',
    allowed_models_mode: 'unrestricted',
    allowed_providers: [],
    allowed_api_formats: [],
    allowed_models: [],
    rate_limit_mode: 'system',
    rate_limit: undefined,
  }
}

function startCreate(): void {
  editingGroupId.value = null
  form.value = createEmptyForm()
  memberUserIds.value = []
}

async function toggleDefault(): Promise<void> {
  const group = selectedGroup.value
  if (!group || group.is_default) return
  const confirmed = await confirmInfo(
    locale.value === 'en-US'
      ? `Set "${group.name}" as the default registration group? Locally registered users and OAuth-created users will join this group.`
      : `确定将「${group.name}」设为默认注册组吗？后续本地注册和 OAuth 自动创建的用户将加入该分组。`,
    legacyT('设为默认注册组'),
  )
  if (!confirmed) return
  saving.value = true
  try {
    await usersStore.setDefaultUserGroup(group.id)
    success(legacyT('已更新默认注册组'))
    emit('changed')
    await loadDialogData()
  } catch (err) {
    error(parseApiError(err, '设置默认注册组失败'), legacyT('设置默认注册组失败'))
  } finally {
    saving.value = false
  }
}

function buildPayload(): UpsertUserGroupRequest {
  return {
    name: form.value.name.trim(),
    allowed_providers_mode: form.value.allowed_providers_mode,
    allowed_api_formats_mode: form.value.allowed_api_formats_mode,
    allowed_models_mode: form.value.allowed_models_mode,
    allowed_providers: form.value.allowed_providers_mode === 'specific'
      ? [...form.value.allowed_providers]
      : null,
    allowed_api_formats: form.value.allowed_api_formats_mode === 'specific'
      ? [...form.value.allowed_api_formats]
      : null,
    allowed_models: form.value.allowed_models_mode === 'specific'
      ? [...form.value.allowed_models]
      : null,
    rate_limit_mode: form.value.rate_limit_mode,
    rate_limit: form.value.rate_limit_mode === 'custom'
      ? (form.value.rate_limit ?? 0)
      : null,
  }
}

async function saveGroup(): Promise<void> {
  if (!form.value.name.trim()) return
  saving.value = true
  try {
    const saved = editingGroupId.value
      ? await usersStore.updateUserGroup(editingGroupId.value, buildPayload())
      : await usersStore.createUserGroup(buildPayload())
    if (!saved.is_default) {
      await usersStore.replaceUserGroupMembers(saved.id, memberUserIds.value)
    }
    success(legacyT('用户分组已保存'))
    emit('changed')
    editingGroupId.value = saved.id
    await loadDialogData()
  } catch (err) {
    error(parseApiError(err, '保存用户分组失败'), legacyT('保存用户分组失败'))
  } finally {
    saving.value = false
  }
}

async function deleteSelectedGroup(): Promise<void> {
  if (!selectedGroup.value) return
  const group = selectedGroup.value
  const confirmed = await confirmDanger(
    locale.value === 'en-US'
      ? `Delete user group ${group.name}? Member relationships will be cleaned up as well.`
      : `确定要删除用户分组 ${group.name} 吗？成员关系会一并清理。`,
    legacyT('删除用户分组'),
  )
  if (!confirmed) return
  saving.value = true
  try {
    await usersStore.deleteUserGroup(group.id)
    success(legacyT('用户分组已删除'))
    emit('changed')
    editingGroupId.value = null
    await loadDialogData()
  } catch (err) {
    error(parseApiError(err, '删除用户分组失败'), legacyT('删除用户分组失败'))
  } finally {
    saving.value = false
  }
}
</script>
