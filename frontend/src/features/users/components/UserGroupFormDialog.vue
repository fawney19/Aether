<template>
  <Dialog
    :model-value="isOpen"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-primary/10">
            <Users class="h-5 w-5 text-primary" />
          </div>
          <div class="min-w-0 flex-1">
            <h3 class="text-lg font-semibold leading-tight text-foreground">
              {{ isEditMode ? '编辑用户分组' : '新增用户分组' }}
            </h3>
            <p class="text-xs text-muted-foreground">
              配置该分组的调度策略、模型分组顺序与访问限制
            </p>
          </div>
        </div>
      </div>
    </template>

    <form @submit.prevent="handleSubmit">
      <div class="grid grid-cols-2 gap-0">
        <div class="space-y-4 pr-6">
          <div class="flex items-center gap-2 border-b border-border/60 pb-2">
            <span class="text-sm font-medium">基础信息</span>
          </div>

          <div class="space-y-2">
            <Label
              for="group-name"
              class="text-sm font-medium"
            >分组名称 <span class="text-muted-foreground">*</span></Label>
            <Input
              id="group-name"
              v-model="form.name"
              type="text"
              class="h-10"
              :class="nameError ? 'border-destructive' : ''"
            />
            <p
              v-if="nameError"
              class="text-xs text-destructive"
            >
              {{ nameError }}
            </p>
            <p
              v-else
              class="text-xs text-muted-foreground"
            >
              用于区分不同用户群体，例如内部、外部、合作方
            </p>
          </div>

          <div class="space-y-2">
            <Label
              for="group-description"
              class="text-sm font-medium"
            >分组描述</Label>
            <Textarea
              id="group-description"
              v-model="form.description"
              rows="8"
              placeholder="可选，描述这个分组的用途和适用范围"
            />
          </div>
        </div>

        <div class="space-y-4 border-l border-border pl-6">
          <div class="flex items-center gap-2 border-b border-border/60 pb-2">
            <span class="text-sm font-medium">访问策略</span>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">调度策略</Label>
            <div class="rounded-xl border border-border/60 bg-muted/20 p-1">
              <div class="grid grid-cols-3 gap-1">
                <button
                  v-for="option in schedulingModeOptions"
                  :key="option.value"
                  type="button"
                  class="rounded-lg px-3 py-2 text-center transition-all duration-200"
                  :class="form.scheduling_mode === option.value
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:bg-background/70 hover:text-foreground'"
                  @click="form.scheduling_mode = option.value"
                >
                  <div class="text-sm font-medium">
                    {{ option.label }}
                  </div>
                </button>
              </div>
            </div>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">模型分组绑定顺序</Label>
            <div class="flex gap-2">
              <Select v-model="pendingModelGroupId">
                <SelectTrigger class="h-10 flex-1">
                  <SelectValue placeholder="选择要绑定的模型分组" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="option in availableModelGroupOptions"
                    :key="option.id"
                    :value="option.id"
                  >
                    {{ option.display_name }}
                  </SelectItem>
                </SelectContent>
              </Select>
              <Button
                type="button"
                variant="outline"
                class="h-10 px-3"
                :disabled="!pendingModelGroupId"
                @click="addModelGroupBinding"
              >
                添加
              </Button>
            </div>
            <div
              v-if="form.model_group_bindings.length === 0"
              class="rounded-lg border border-dashed border-border/60 px-3 py-4 text-xs text-muted-foreground"
            >
              未显式绑定模型分组时，保存后会自动绑定系统模型分组。
            </div>

            <div
              v-else
              class="space-y-2"
            >

              <div
                v-for="(binding, index) in form.model_group_bindings"
                :key="binding.model_group_id"
                class="group flex items-center gap-3 rounded-xl border px-3 py-2.5 transition-all duration-200"
                :class="draggedBindingIndex === index
                  ? 'border-primary/50 bg-primary/5 shadow-md scale-[1.01]'
                  : dragOverBindingIndex === index
                    ? 'border-primary/30 bg-primary/5'
                    : 'border-border/60 bg-muted/15 hover:border-border hover:bg-muted/25'"
                draggable="true"
                @dragstart="handleBindingDragStart(index, $event)"
                @dragend="handleBindingDragEnd"
                @dragover.prevent="handleBindingDragOver(index)"
                @dragleave="handleBindingDragLeave"
                @drop="handleBindingDrop(index)"
              >
                <div class="cursor-grab rounded-md p-1 text-muted-foreground/40 transition-colors group-hover:text-muted-foreground active:cursor-grabbing">
                  <GripVertical class="h-4 w-4" />
                </div>

                <Badge
                  variant="secondary"
                  class="h-6 min-w-6 shrink-0 justify-center px-1.5 py-0 text-[10px]"
                >
                  {{ index + 1 }}
                </Badge>

                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2">
                    <span
                      class="truncate text-sm font-medium"
                      :class="binding.is_active ? 'text-foreground' : 'text-muted-foreground'"
                    >
                      {{ getModelGroupLabel(binding.model_group_id) }}
                    </span>
                    <span
                      v-if="!binding.is_active"
                      class="shrink-0 text-[11px] text-muted-foreground"
                    >
                      已停用
                    </span>
                  </div>
                </div>

                <Switch
                  :model-value="binding.is_active"
                  @update:model-value="(value) => updateBindingActive(index, value)"
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 shrink-0 text-rose-600 hover:bg-rose-50 hover:text-rose-600 dark:hover:bg-rose-950/30"
                  @click="removeBinding(index)"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">API 格式</Label>
            <div class="flex items-center gap-3">
              <div class="min-w-0 flex-1">
                <MultiSelect
                  v-model="form.allowed_api_formats"
                  :options="apiFormatOptions"
                  :search-threshold="0"
                  :disabled="form.api_format_unrestricted"
                  :placeholder="form.api_format_unrestricted ? '不限制' : '未选择（全部禁用）'"
                  empty-text="暂无可用 API 格式"
                  no-results-text="未找到匹配的 API 格式"
                  search-placeholder="搜索 API 格式..."
                />
              </div>
              <Switch
                v-model="form.api_format_unrestricted"
                class="shrink-0"
              />
            </div>
          </div>

          <div class="space-y-2">
            <Label
              for="group-rate-limit"
              class="text-sm font-medium"
            >速率限制 (请求/分钟)</Label>
            <div class="flex items-center gap-3">
              <div class="min-w-0 flex-1">
                <Input
                  v-if="!form.rate_limit_inherited"
                  id="group-rate-limit"
                  :model-value="form.rate_limit ?? ''"
                  type="number"
                  min="0"
                  max="10000"
                  placeholder="0 = 不限速"
                  class="h-10"
                  @update:model-value="(v) => form.rate_limit = parseNumberInput(v, { min: 0, max: 10000 })"
                />
                <span
                  v-else
                  class="flex h-10 w-full items-center rounded-lg border bg-background px-3 text-sm text-muted-foreground opacity-60"
                >跟随系统</span>
              </div>
              <Switch
                v-model="form.rate_limit_inherited"
                class="shrink-0"
              />
            </div>
          </div>
        </div>
      </div>
    </form>

    <template #footer>
      <Button
        variant="outline"
        type="button"
        class="h-10 px-5"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        class="h-10 px-5"
        :disabled="saving || !isFormValid"
        @click="handleSubmit"
      >
        {{ saving ? '处理中...' : isEditMode ? '更新' : '创建' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  GripVertical,
  Trash2,
  Users,
} from 'lucide-vue-next'
import {
  Badge,
  Button,
  Dialog,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  Textarea,
} from '@/components/ui'
import { MultiSelect } from '@/components/common'
import { useFormDialog } from '@/composables/useFormDialog'
import { adminApi } from '@/api/admin'
import { modelGroupsApi, type ModelGroupSummary } from '@/api/model-groups'
import type { UserGroupSchedulingMode } from '@/api/users'
import { log } from '@/utils/logger'
import { parseNumberInput } from '@/utils/form'

export interface UserGroupFormBinding {
  model_group_id: string
  priority: number
  is_active: boolean
  model_group_name?: string | null
  model_group_display_name?: string | null
  model_group_is_default?: boolean
}

export interface UserGroupFormData {
  id?: string
  name: string
  description?: string | null
  scheduling_mode?: UserGroupSchedulingMode
  allowed_api_formats?: string[] | null
  model_group_bindings?: UserGroupFormBinding[] | null
  rate_limit?: number | null
}

const props = defineProps<{
  open: boolean
  group: UserGroupFormData | null
}>()

const emit = defineEmits<{
  close: []
  submit: [data: UserGroupFormData]
}>()

const saving = ref(false)
const modelGroups = ref<ModelGroupSummary[]>([])
const apiFormats = ref<Array<{ value: string; label: string }>>([])
const pendingModelGroupId = ref('')
const draggedBindingIndex = ref<number | null>(null)
const dragOverBindingIndex = ref<number | null>(null)

const schedulingModeOptions: Array<{
  value: UserGroupSchedulingMode
  label: string
}> = [
  { value: 'cache_affinity', label: '缓存亲和' },
  { value: 'load_balance', label: '负载均衡' },
  { value: 'fixed_order', label: '固定顺序' },
]

const apiFormatOptions = computed(() =>
  apiFormats.value.map((format) => ({
    value: format.value,
    label: format.label,
  })),
)

const availableModelGroupOptions = computed(() => {
  const selected = new Set(form.value.model_group_bindings.map((item) => item.model_group_id))
  return modelGroups.value.filter((group) => !selected.has(group.id))
})

const defaultModelGroup = computed(() => {
  return modelGroups.value.find((group) => group.is_default) || null
})

const form = ref({
  name: '',
  description: '',
  scheduling_mode: 'cache_affinity' as UserGroupSchedulingMode,
  api_format_unrestricted: true,
  rate_limit_inherited: true,
  allowed_api_formats: [] as string[],
  model_group_bindings: [] as UserGroupFormBinding[],
  rate_limit: undefined as number | undefined,
})

function reindexBindings() {
  form.value.model_group_bindings = form.value.model_group_bindings.map((binding, index) => ({
    ...binding,
    priority: (index + 1) * 10,
  }))
}

function cloneBindings(bindings: UserGroupFormBinding[] | null | undefined): UserGroupFormBinding[] {
  return [...(bindings || [])]
    .map((binding) => ({
      model_group_id: binding.model_group_id,
      priority: binding.priority,
      is_active: binding.is_active,
      model_group_name: binding.model_group_name ?? null,
      model_group_display_name: binding.model_group_display_name ?? null,
      model_group_is_default: binding.model_group_is_default ?? false,
    }))
    .sort((a, b) => a.priority - b.priority)
}

function resetForm() {
  pendingModelGroupId.value = ''
  draggedBindingIndex.value = null
  dragOverBindingIndex.value = null
  form.value = {
    name: '',
    description: '',
    scheduling_mode: 'cache_affinity',
    api_format_unrestricted: true,
    rate_limit_inherited: true,
    allowed_api_formats: [],
    model_group_bindings: [],
    rate_limit: undefined,
  }
}

function ensureCreateDefaultBinding() {
  if (props.group || form.value.model_group_bindings.length > 0 || !defaultModelGroup.value) {
    return
  }
  form.value.model_group_bindings = [
    {
      model_group_id: defaultModelGroup.value.id,
      priority: 10,
      is_active: true,
      model_group_name: defaultModelGroup.value.name,
      model_group_display_name: defaultModelGroup.value.display_name,
      model_group_is_default: defaultModelGroup.value.is_default,
    },
  ]
}

function loadGroupData() {
  pendingModelGroupId.value = ''
  draggedBindingIndex.value = null
  dragOverBindingIndex.value = null
  if (!props.group) {
    resetForm()
    ensureCreateDefaultBinding()
    return
  }
  form.value = {
    name: props.group.name,
    description: props.group.description ?? '',
    scheduling_mode: props.group.scheduling_mode ?? 'cache_affinity',
    api_format_unrestricted: props.group.allowed_api_formats == null,
    rate_limit_inherited: props.group.rate_limit == null,
    allowed_api_formats: props.group.allowed_api_formats ? [...props.group.allowed_api_formats] : [],
    model_group_bindings: cloneBindings(props.group.model_group_bindings),
    rate_limit: props.group.rate_limit ?? undefined,
  }
  if (form.value.model_group_bindings.length === 0) {
    ensureCreateDefaultBinding()
  } else {
    reindexBindings()
  }
}

const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.group,
  isLoading: saving,
  onClose: () => emit('close'),
  loadData: loadGroupData,
  resetForm,
})

const isOpen = computed(() => props.open)
const nameError = computed(() => {
  const value = form.value.name.trim()
  if (!value) return '分组名称不能为空'
  if (value.length > 100) return '分组名称不能超过 100 个字符'
  return ''
})
const isFormValid = computed(() => !nameError.value)

function getModelGroupMeta(modelGroupId: string): ModelGroupSummary | undefined {
  return modelGroups.value.find((group) => group.id === modelGroupId)
}

function getModelGroupLabel(modelGroupId: string): string {
  const binding = form.value.model_group_bindings.find((item) => item.model_group_id === modelGroupId)
  if (binding?.model_group_display_name) return binding.model_group_display_name
  if (binding?.model_group_name) return binding.model_group_name
  const meta = getModelGroupMeta(modelGroupId)
  return meta?.display_name || meta?.name || modelGroupId
}

function addModelGroupBinding() {
  const modelGroupId = pendingModelGroupId.value
  if (!modelGroupId) return
  const meta = getModelGroupMeta(modelGroupId)
  form.value.model_group_bindings.push({
    model_group_id: modelGroupId,
    priority: 0,
    is_active: true,
    model_group_name: meta?.name ?? null,
    model_group_display_name: meta?.display_name ?? null,
    model_group_is_default: meta?.is_default ?? false,
  })
  pendingModelGroupId.value = ''
  reindexBindings()
}

function handleBindingDragStart(index: number, event: DragEvent) {
  draggedBindingIndex.value = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', '')
  }
}

function handleBindingDragEnd() {
  draggedBindingIndex.value = null
  dragOverBindingIndex.value = null
}

function handleBindingDragOver(index: number) {
  if (draggedBindingIndex.value !== null && draggedBindingIndex.value !== index) {
    dragOverBindingIndex.value = index
  }
}

function handleBindingDragLeave() {
  dragOverBindingIndex.value = null
}

function handleBindingDrop(targetIndex: number) {
  const dragIndex = draggedBindingIndex.value
  if (dragIndex === null || dragIndex === targetIndex) {
    dragOverBindingIndex.value = null
    return
  }

  const items = [...form.value.model_group_bindings]
  const [draggedItem] = items.splice(dragIndex, 1)
  items.splice(targetIndex, 0, draggedItem)
  form.value.model_group_bindings = items
  reindexBindings()
  draggedBindingIndex.value = null
  dragOverBindingIndex.value = null
}

function removeBinding(index: number) {
  form.value.model_group_bindings.splice(index, 1)
  reindexBindings()
}

function updateBindingActive(index: number, value: boolean) {
  form.value.model_group_bindings[index] = {
    ...form.value.model_group_bindings[index],
    is_active: !!value,
  }
}

function hydrateBindingMeta() {
  if (form.value.model_group_bindings.length === 0) return

  form.value.model_group_bindings = form.value.model_group_bindings.map((binding) => {
    const meta = getModelGroupMeta(binding.model_group_id)
    return {
      ...binding,
      model_group_name: binding.model_group_name ?? meta?.name ?? null,
      model_group_display_name: binding.model_group_display_name ?? meta?.display_name ?? null,
      model_group_is_default: binding.model_group_is_default ?? meta?.is_default ?? false,
    }
  })
}

async function loadAccessControlOptions(): Promise<void> {
  try {
    const [groups, formatsData] = await Promise.all([
      modelGroupsApi.list(),
      adminApi.getApiFormats(),
    ])
    modelGroups.value = groups
    apiFormats.value = formatsData.formats || []
    hydrateBindingMeta()
    ensureCreateDefaultBinding()
  } catch (err) {
    log.error('加载用户分组选项失败:', err)
  }
}

async function handleSubmit() {
  if (!isFormValid.value) return
  saving.value = true
  try {
    emit('submit', {
      id: props.group?.id,
      name: form.value.name.trim(),
      description: form.value.description.trim() || null,
      scheduling_mode: form.value.scheduling_mode,
      allowed_api_formats: form.value.api_format_unrestricted ? null : [...form.value.allowed_api_formats],
      model_group_bindings: form.value.model_group_bindings.length > 0
        ? form.value.model_group_bindings.map((binding, index) => ({
          model_group_id: binding.model_group_id,
          priority: (index + 1) * 10,
          is_active: binding.is_active,
          model_group_name: binding.model_group_name ?? null,
          model_group_display_name: binding.model_group_display_name ?? null,
          model_group_is_default: binding.model_group_is_default ?? false,
        }))
        : [],
      rate_limit: form.value.rate_limit_inherited ? null : (form.value.rate_limit ?? 0),
    })
  } finally {
    saving.value = false
  }
}

function setSaving(value: boolean) {
  saving.value = value
}

watch(isOpen, (val) => {
  if (val) {
    void loadAccessControlOptions()
  }
})

defineExpose({
  setSaving,
})
</script>
