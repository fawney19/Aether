<template>
  <Dialog
    :model-value="open"
    title="用户分组"
    description="管理用户组、默认注册组、成员和组级访问控制"
    size="4xl"
    persistent
    @update:model-value="handleDialogUpdate"
  >
    <div class="grid gap-4 lg:min-h-[560px] lg:grid-cols-[17rem_minmax(0,1fr)]">
      <div class="rounded-xl border border-border/70 bg-muted/20 p-3">
        <div class="mb-3 flex items-center justify-between gap-2">
          <Label class="text-sm font-semibold">分组</Label>
          <Button
            variant="ghost"
            size="icon"
            class="h-8 w-8"
            title="新建分组"
            @click="startCreate"
          >
            <Plus class="h-4 w-4" />
          </Button>
        </div>

        <div
          v-if="loading"
          class="rounded-lg border border-dashed border-border/70 px-3 py-8 text-center text-xs text-muted-foreground"
        >
          正在加载...
        </div>
        <div
          v-else-if="groups.length === 0"
          class="rounded-lg border border-dashed border-border/70 px-3 py-8 text-center text-xs text-muted-foreground"
        >
          暂无分组
        </div>
        <div
          v-else
          class="max-h-60 space-y-1.5 overflow-y-auto lg:max-h-none lg:overflow-visible"
        >
          <button
            v-for="group in groups"
            :key="group.id"
            type="button"
            :class="groupButtonClass(group.id)"
            @click="selectGroup(group.id)"
          >
            <span class="min-w-0 flex-1 text-left">
              <span class="flex items-center gap-1.5">
                <span class="truncate text-sm font-medium">{{ group.name }}</span>
                <Badge
                  v-if="group.is_default"
                  variant="secondary"
                  class="h-5 px-1.5 py-0 text-[10px]"
                >
                  默认
                </Badge>
              </span>
            </span>
            <ChevronRight class="h-4 w-4 shrink-0 text-muted-foreground" />
          </button>
        </div>
      </div>

      <div class="min-w-0 rounded-xl border border-border/70 bg-background p-3 sm:p-4">
        <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
          <div class="min-w-0">
            <h4 class="truncate text-base font-semibold text-foreground">
              {{ editingGroupId ? '编辑分组' : '新建分组' }}
            </h4>
            <p class="text-xs text-muted-foreground">
              {{ selectedGroup?.is_default ? '当前为所有用户的默认组' : '通过额外分组配置访问限制' }}
            </p>
          </div>
          <div
            v-if="editingGroupId"
            class="flex items-center gap-1"
          >
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              :class="selectedGroup?.is_default ? 'text-emerald-500 hover:text-emerald-500' : ''"
              :disabled="saving || selectedGroup?.is_default"
              :title="selectedGroup?.is_default ? '默认注册组' : '设为默认注册组'"
              @click="toggleDefault"
            >
              <BadgeCheck class="h-4 w-4" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              :disabled="saving || selectedGroup?.is_default"
              title="删除分组"
              @click="deleteSelectedGroup"
            >
              <Trash2 class="h-4 w-4" />
            </Button>
          </div>
        </div>

        <div class="space-y-5">
          <div class="space-y-4">
            <div class="space-y-2">
              <Label class="text-sm font-medium">名称</Label>
              <Input
                v-model="form.name"
                class="h-10"
                placeholder="例如：生产团队"
              />
            </div>

            <div class="space-y-2">
              <Label class="text-sm font-medium">成员</Label>
              <MultiSelect
                v-model="memberUserIds"
                :options="userOptions"
                :search-threshold="0"
                :disabled="selectedGroup?.is_default"
                placeholder="选择用户"
                empty-text="暂无用户"
                no-results-text="未找到匹配用户"
              />
            </div>
          </div>

          <div class="space-y-4 border-t border-border/60 pt-5">
            <div class="flex flex-wrap items-baseline justify-between gap-x-2 gap-y-1 pb-2 border-b border-border/60">
              <span class="text-sm font-medium">组权限</span>
              <span class="flex items-center gap-1 text-[11px] text-muted-foreground">
                组权限叠加，Key 可再收窄
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger as-child>
                      <button
                        type="button"
                        class="inline-flex h-4 w-4 items-center justify-center rounded-full border border-border/70 bg-muted/40 text-muted-foreground outline-none transition-colors hover:border-primary/50 hover:text-primary focus-visible:border-primary/60 focus-visible:text-primary"
                        :title="groupPolicyHelpText"
                        aria-label="查看组权限合并规则"
                      >
                        <Info class="h-3 w-3" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent class="max-w-72 text-xs leading-5">
                      {{ groupPolicyHelpText }}
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </span>
            </div>

            <div class="space-y-2">
              <Label class="text-sm font-medium">允许的提供商</Label>
              <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
                <div class="flex w-full items-center sm:w-auto sm:shrink-0">
                  <Switch
                    :model-value="form.allowed_providers_mode === 'unrestricted'"
                    @update:model-value="(v) => (form.allowed_providers_mode = v ? 'unrestricted' : 'specific')"
                  />
                </div>
                <div class="min-w-0 flex-1">
                  <MultiSelect
                    v-model="form.allowed_providers"
                    :options="providerOptions"
                    :search-threshold="0"
                    :disabled="form.allowed_providers_mode === 'unrestricted'"
                    :placeholder="form.allowed_providers_mode === 'unrestricted' ? '不限制所有选项' : '选择提供商'"
                    empty-text="暂无选项"
                  />
                </div>
              </div>
            </div>

            <div class="space-y-2">
              <Label class="text-sm font-medium">允许的端点</Label>
              <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
                <div class="flex w-full items-center sm:w-auto sm:shrink-0">
                  <Switch
                    :model-value="form.allowed_api_formats_mode === 'unrestricted'"
                    @update:model-value="(v) => (form.allowed_api_formats_mode = v ? 'unrestricted' : 'specific')"
                  />
                </div>
                <div class="min-w-0 flex-1">
                  <MultiSelect
                    v-model="form.allowed_api_formats"
                    :options="apiFormatOptions"
                    :search-threshold="0"
                    :disabled="form.allowed_api_formats_mode === 'unrestricted'"
                    :placeholder="form.allowed_api_formats_mode === 'unrestricted' ? '不限制所有选项' : '选择端点'"
                    empty-text="暂无选项"
                  />
                </div>
              </div>
            </div>

            <div class="space-y-2">
              <Label class="text-sm font-medium">允许的模型</Label>
              <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
                <div class="flex w-full items-center sm:w-auto sm:shrink-0">
                  <Switch
                    :model-value="form.allowed_models_mode === 'unrestricted'"
                    @update:model-value="(v) => (form.allowed_models_mode = v ? 'unrestricted' : 'specific')"
                  />
                </div>
                <div class="min-w-0 flex-1">
                  <MultiSelect
                    v-model="form.allowed_models"
                    :options="modelOptions"
                    :search-threshold="0"
                    :disabled="form.allowed_models_mode === 'unrestricted'"
                    :placeholder="form.allowed_models_mode === 'unrestricted' ? '不限制所有选项' : '选择模型'"
                    empty-text="暂无选项"
                  />
                </div>
              </div>
            </div>

            <div class="space-y-2">
              <Label class="text-sm font-medium">速率限制 (请求/分钟)</Label>
              <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
                <div class="flex w-full items-center sm:w-auto sm:shrink-0">
                  <Switch
                    :model-value="form.rate_limit_mode === 'system'"
                    @update:model-value="(v) => (form.rate_limit_mode = v ? 'system' : 'custom')"
                  />
                </div>
                <div class="min-w-0 flex-1">
                  <Input
                    :model-value="form.rate_limit ?? ''"
                    type="number"
                    min="0"
                    max="10000"
                    class="h-10"
                    :disabled="form.rate_limit_mode === 'system'"
                    :placeholder="form.rate_limit_mode === 'system' ? '使用系统默认' : '0 = 不限速'"
                    @update:model-value="(value) => form.rate_limit = parseNumberInput(value, { min: 0, max: 10000 })"
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="saving"
        @click="emit('close')"
      >
        关闭
      </Button>
      <Button
        :disabled="saving || !form.name.trim()"
        @click="saveGroup"
      >
        保存
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { BadgeCheck, ChevronRight, Info, Plus, Trash2 } from 'lucide-vue-next'
import {
  Badge,
  Button,
  Dialog,
  Input,
  Label,
  Switch,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui'
import { MultiSelect } from '@/components/common'
import { useUsersStore } from '@/stores/users'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import { parseNumberInput } from '@/utils/form'
import { cn } from '@/lib/utils'
import { useUserAccessControlOptions } from '@/features/users/composables/useUserAccessControlOptions'
import type {
  ListPolicyMode,
  RateLimitPolicyMode,
  UpsertUserGroupRequest,
  User,
  UserGroup,
} from '@/api/users'

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

const form = ref({
  name: '',
  allowed_providers_mode: 'unrestricted' as ListPolicyMode,
  allowed_api_formats_mode: 'unrestricted' as ListPolicyMode,
  allowed_models_mode: 'unrestricted' as ListPolicyMode,
  allowed_providers: [] as string[],
  allowed_api_formats: [] as string[],
  allowed_models: [] as string[],
  rate_limit_mode: 'system' as RateLimitPolicyMode,
  rate_limit: undefined as number | undefined,
})

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
      error(parseApiError(err, '加载访问控制选项失败'))
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
    error(parseApiError(err, '加载用户分组失败'))
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
    error(parseApiError(err, '加载分组成员失败'))
  }
}

function normalizeListMode(mode: ListPolicyMode): ListPolicyMode {
  return mode === 'specific' ? 'specific' : 'unrestricted'
}

function normalizeRateMode(mode: RateLimitPolicyMode): RateLimitPolicyMode {
  return mode === 'custom' ? 'custom' : 'system'
}

function startCreate(): void {
  editingGroupId.value = null
  form.value = {
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
  memberUserIds.value = []
}

function groupButtonClass(groupId: string): string {
  return cn(
    'flex w-full items-center gap-2 rounded-lg border px-3 py-2 transition-colors',
    editingGroupId.value === groupId
      ? 'border-primary/50 bg-primary/10'
      : 'border-transparent hover:border-border hover:bg-background',
  )
}

async function toggleDefault(): Promise<void> {
  const group = selectedGroup.value
  if (!group || group.is_default) return
  const confirmed = await confirmInfo(
    `确定将「${group.name}」设为默认注册组吗？后续本地注册和 OAuth 自动创建的用户将加入该分组。`,
    '设为默认注册组',
  )
  if (!confirmed) return
  saving.value = true
  try {
    await usersStore.setDefaultUserGroup(group.id)
    success('已更新默认注册组')
    emit('changed')
    await loadDialogData()
  } catch (err) {
    error(parseApiError(err, '设置默认注册组失败'))
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
    success('用户分组已保存')
    emit('changed')
    editingGroupId.value = saved.id
    await loadDialogData()
  } catch (err) {
    error(parseApiError(err, '保存用户分组失败'))
  } finally {
    saving.value = false
  }
}

async function deleteSelectedGroup(): Promise<void> {
  if (!selectedGroup.value) return
  const group = selectedGroup.value
  const confirmed = await confirmDanger(
    `确定要删除用户分组 ${group.name} 吗？成员关系会一并清理。`,
    '删除用户分组',
  )
  if (!confirmed) return
  saving.value = true
  try {
    await usersStore.deleteUserGroup(group.id)
    success('用户分组已删除')
    emit('changed')
    editingGroupId.value = null
    await loadDialogData()
  } catch (err) {
    error(parseApiError(err, '删除用户分组失败'))
  } finally {
    saving.value = false
  }
}
</script>
