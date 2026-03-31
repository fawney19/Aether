<template>
  <Dialog
    :model-value="isOpen"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
            <Users class="h-5 w-5 text-primary" />
          </div>
          <div class="flex-1 min-w-0">
            <h3 class="text-lg font-semibold text-foreground leading-tight">
              {{ isEditMode ? '编辑用户分组' : '新增用户分组' }}
            </h3>
            <p class="text-xs text-muted-foreground">
              配置该分组的默认访问限制
            </p>
          </div>
        </div>
      </div>
    </template>

    <form @submit.prevent="handleSubmit">
      <div class="grid grid-cols-2 gap-0">
        <div class="pr-6 space-y-4">
          <div class="flex items-center gap-2 pb-2 border-b border-border/60">
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

        <div class="pl-6 space-y-4 border-l border-border">
          <div class="flex items-center gap-2 pb-2 border-b border-border/60">
            <span class="text-sm font-medium">默认限制</span>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">默认 Provider</Label>
            <div class="flex items-center gap-3">
              <div class="flex-1 min-w-0">
                <MultiSelect
                  v-model="form.allowed_providers"
                  :options="providerOptions"
                  :search-threshold="0"
                  :disabled="form.provider_unrestricted"
                  :placeholder="form.provider_unrestricted ? '不限制' : '未选择（全部禁用）'"
                  empty-text="暂无可用 Provider"
                  no-results-text="未找到匹配的 Provider"
                  search-placeholder="搜索 Provider 名称..."
                />
              </div>
              <Switch
                v-model="form.provider_unrestricted"
                class="shrink-0"
              />
            </div>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">默认 API 格式</Label>
            <div class="flex items-center gap-3">
              <div class="flex-1 min-w-0">
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
            <Label class="text-sm font-medium">默认模型</Label>
            <div class="flex items-center gap-3">
              <div class="flex-1 min-w-0">
                <MultiSelect
                  v-model="form.allowed_models"
                  :options="modelOptions"
                  :search-threshold="0"
                  :disabled="form.model_unrestricted"
                  :placeholder="form.model_unrestricted ? '不限制' : '未选择（全部禁用）'"
                  empty-text="暂无可用模型"
                  no-results-text="未找到匹配的模型"
                  search-placeholder="输入模型名搜索..."
                />
              </div>
              <Switch
                v-model="form.model_unrestricted"
                class="shrink-0"
              />
            </div>
          </div>

          <div class="space-y-2">
            <Label
              for="group-rate-limit"
              class="text-sm font-medium"
            >默认速率限制 (请求/分钟)</Label>
            <div class="flex items-center gap-3">
              <div class="flex-1 min-w-0">
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
                >跟随系统默认</span>
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
import { Users } from 'lucide-vue-next'
import {
  Button,
  Dialog,
  Input,
  Label,
  Switch,
  Textarea,
} from '@/components/ui'
import { MultiSelect } from '@/components/common'
import { useFormDialog } from '@/composables/useFormDialog'
import { getProvidersSummary } from '@/api/endpoints/providers'
import { getGlobalModels } from '@/api/global-models'
import { adminApi } from '@/api/admin'
import { log } from '@/utils/logger'
import { parseNumberInput } from '@/utils/form'
import type {
  ProviderWithEndpointsSummary,
  GlobalModelResponse,
} from '@/api/endpoints/types'

export interface UserGroupFormData {
  id?: string
  name: string
  description?: string | null
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
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

const providers = ref<ProviderWithEndpointsSummary[]>([])
const globalModels = ref<GlobalModelResponse[]>([])
const apiFormats = ref<Array<{ value: string; label: string }>>([])

const providerOptions = computed(() =>
  providers.value.map((provider) => ({
    value: provider.id,
    label: provider.name,
  })),
)
const apiFormatOptions = computed(() =>
  apiFormats.value.map((format) => ({
    value: format.value,
    label: format.label,
  })),
)
const modelOptions = computed(() =>
  globalModels.value.map((model) => ({
    value: model.name,
    label: model.name,
  })),
)

const form = ref({
  name: '',
  description: '',
  provider_unrestricted: true,
  api_format_unrestricted: true,
  model_unrestricted: true,
  rate_limit_inherited: true,
  allowed_providers: [] as string[],
  allowed_api_formats: [] as string[],
  allowed_models: [] as string[],
  rate_limit: undefined as number | undefined,
})

function resetForm() {
  form.value = {
    name: '',
    description: '',
    provider_unrestricted: true,
    api_format_unrestricted: true,
    model_unrestricted: true,
    rate_limit_inherited: true,
    allowed_providers: [],
    allowed_api_formats: [],
    allowed_models: [],
    rate_limit: undefined,
  }
}

function loadGroupData() {
  if (!props.group) return
  form.value = {
    name: props.group.name,
    description: props.group.description ?? '',
    provider_unrestricted: props.group.allowed_providers == null,
    api_format_unrestricted: props.group.allowed_api_formats == null,
    model_unrestricted: props.group.allowed_models == null,
    rate_limit_inherited: props.group.rate_limit == null,
    allowed_providers: props.group.allowed_providers ? [...props.group.allowed_providers] : [],
    allowed_api_formats: props.group.allowed_api_formats ? [...props.group.allowed_api_formats] : [],
    allowed_models: props.group.allowed_models ? [...props.group.allowed_models] : [],
    rate_limit: props.group.rate_limit ?? undefined,
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

async function loadAccessControlOptions(): Promise<void> {
  try {
    const [providersResponse, modelsData, formatsData] = await Promise.all([
      getProvidersSummary({ page_size: 9999 }),
      getGlobalModels({ limit: 1000, is_active: true }),
      adminApi.getApiFormats(),
    ])
    providers.value = providersResponse.items
    globalModels.value = modelsData.models || []
    apiFormats.value = formatsData.formats || []
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
      allowed_providers: form.value.provider_unrestricted ? null : [...form.value.allowed_providers],
      allowed_api_formats: form.value.api_format_unrestricted ? null : [...form.value.allowed_api_formats],
      allowed_models: form.value.model_unrestricted ? null : [...form.value.allowed_models],
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
    loadAccessControlOptions()
  }
})

defineExpose({
  setSaving,
})
</script>
